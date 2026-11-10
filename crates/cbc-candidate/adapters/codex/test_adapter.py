#!/usr/bin/env python3
"""Process-level tests: Codex MCP adapter -> fake core.

The adapter runs as a real subprocess on stdio, the way Codex spawns it, and the
core is a fake that speaks the Rust v1 wire and rejects unknown fields. Host
metadata here is synthetic, so these tests say nothing about what Codex sends;
binding_probe.py is the evidence for that.
Run: python3 -I test_adapter.py -v
"""
import json, os, queue, secrets, socket, subprocess, sys, tempfile, threading, time, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ADAPTER = os.path.join(HERE, "adapter.py")
THREAD_A, THREAD_B, STRANGER = "thread-alpha", "thread-beta", "thread-stranger"
ROOM = "room-ab"
MAX_BATCH_BYTES = 48 * 1024  # inbox.rs: budget for a new batch
MAX_ESCAPED_BODY = 32 * 1024  # serde_json-escaped body, without its two quotes
sys.dont_write_bytecode = True


def rust_json(value):
    """serde_json's compact encoding: UTF-8 kept, no spaces."""
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def read_line(conn):
    buf = b""
    while not buf.endswith(b"\n"):
        chunk = conn.recv(65536)
        if not chunk:
            break
        buf += chunk
    return buf


class FakeCore:
    """Speaks the Rust connector v1 wire; exact-field checks stand in for deny_unknown_fields."""
    OPS = {"check": set(), "send": {"room", "key", "body"}, "receipt": {"reference"},
           "discover": {"limit"}, "connect": {"target", "kind", "scope_ref", "key", "explanation"},
           "binding_status": set()}

    def __init__(self, path, grants, rooms):
        self.tokens = {g["member"]: g["token"] for g in grants}
        self.rooms = rooms
        self.items, self.pending, self.done, self.keys, self.requests, self.references = [], {}, {}, {}, [], []
        self.operations = []
        self.status_requests = []
        self.native_targets = {g["member"]: None for g in grants}
        self.status_override = None
        self.srv = socket.socket(socket.AF_UNIX)
        self.srv.bind(path)
        self.srv.listen(8)
        threading.Thread(target=self._run, daemon=True).start()

    def _run(self):
        while True:
            try:
                conn, _ = self.srv.accept()
            except OSError:
                return
            with conn:
                try:
                    conn.sendall((rust_json(self.handle(read_line(conn))) + "\n").encode())
                except OSError:
                    pass

    def close(self):
        self.srv.close()

    def handle(self, raw):
        try:
            req = json.loads(raw)
            assert set(req) == {"v", "auth", "request"} and req["v"] == 1
            auth, op = req["auth"], dict(req["request"])
            assert set(auth) == {"context", "member", "device", "session", "epoch", "token"}
            name = op.pop("op")
            assert set(op) == self.OPS[name]
        except (ValueError, AssertionError, KeyError, TypeError):
            return {"type": "error", "message": "invalid candidate request"}
        member = auth["member"]
        if name == "binding_status":
            self.status_requests.append(member)
        else:
            self.requests.append((member, name))
        if self.tokens.get(member) != auth["token"]:
            return {"type": "error", "message": "candidate operation rejected"}
        if name == "binding_status":
            if self.status_override is not None:
                return json.loads(json.dumps(self.status_override))
            return {"type": "binding_status", "native_target": self.native_targets[member]}
        self.operations.append((member, name, op))
        if name == "discover":
            return {"type": "directory", "members": [{"member": other, "label": other}
                for other in self.tokens if other != member][:op["limit"]]}
        if name == "connect":
            return {"type": "connected", "connection": {"room": "connected-route", "kind": op["kind"],
                "scope_ref": op["scope_ref"], "introduction_sequence": 1, "reused": False, "closed": False}}
        done = self.done.setdefault(member, set())
        if name == "send":
            if member not in self.rooms.get(op["room"], ()):
                return {"type": "error", "message": "candidate operation rejected"}
            prior = self.keys.get((member, op["key"]))
            if prior:
                return {"type": "sent", "sequence": prior["sequence"]}
            item = {"sequence": len(self.items) + 1, "room": op["room"], "sender": member,
                    "kind": "message", "body": op["body"]}
            self.items.append(item)
            self.keys[(member, op["key"])] = item
            return {"type": "sent", "sequence": item["sequence"]}
        if name == "check":
            if member not in self.pending:
                items = [i for i in self.items if i["sender"] != member and i["sequence"] not in done
                         and member in self.rooms[i["room"]]]
                if not items:
                    return {"type": "empty"}
                reference = secrets.token_hex(32)
                self.references.append(reference)
                self.pending[member] = {"reference": reference, "items": items[:32]}
            p = self.pending[member]
            return {"type": "batch", "items": p["items"], "reference": p["reference"]}
        if name == "receipt":
            p = self.pending.get(member)
            if not p or p["reference"] != op["reference"]:
                return {"type": "error", "message": "candidate operation rejected"}
            done.update(i["sequence"] for i in p["items"])
            del self.pending[member]
            return {"type": "receipted"}
        return {"type": "error", "message": "candidate operation rejected"}

    def wait_for(self, member, name, timeout=5.0):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if (member, name) in self.requests:
                return True
            time.sleep(0.02)
        return False


def write_private(path, data, mode=0o600):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    with os.fdopen(fd, "w") as f:
        f.write(data)
    os.chmod(path, mode)


class Adapter:
    """One adapter process, driven line by line as a Codex host would."""

    def __init__(self, profile, extra_env=None, host_gone=False):
        env = {"PATH": "/bin:/usr/bin", "LANG": "C"}
        if profile is not None:
            env["CBC_CANDIDATE_PROFILE"] = profile
        env.update(extra_env or {})
        stdout = subprocess.PIPE
        if host_gone:  # a pipe with no reader: every result write fails
            reader, stdout = os.pipe()
            os.close(reader)
        self.proc = subprocess.Popen([sys.executable, "-I", ADAPTER], stdin=subprocess.PIPE,
                                     stdout=stdout, stderr=subprocess.PIPE, env=env)
        self.lines, self.seen, self.next_id = queue.Queue(), [], 0
        if host_gone:
            os.close(stdout)
        else:
            threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        for raw in iter(self.proc.stdout.readline, b""):
            self.seen.append(raw)
            self.lines.put(raw)

    def send_raw(self, data):
        self.proc.stdin.write(data)
        self.proc.stdin.flush()

    def request(self, method, params=None):
        self.next_id += 1
        msg = {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params or {}}
        try:
            line = (rust_json(msg) + "\n").encode()
        except UnicodeEncodeError:  # a lone surrogate only travels as a \uD800-style escape
            line = (json.dumps(msg) + "\n").encode()
        self.send_raw(line)
        return self.next_id

    def reply(self, rid, timeout=5.0):
        while True:
            msg = json.loads(self.lines.get(timeout=timeout))
            if msg.get("id") == rid:
                return msg

    def call(self, name, arguments=None, meta=None):
        params = {"name": name, "arguments": {} if arguments is None else arguments}
        if meta is not None:
            params["_meta"] = meta
        msg = self.reply(self.request("tools/call", params))
        result = msg["result"]
        return json.loads(result["content"][0]["text"]), result["isError"]

    def close(self):
        try:
            self.proc.stdin.close()
        except OSError:
            pass
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
        for stream in (self.proc.stdout, self.proc.stderr):
            try:
                if stream:
                    stream.close()
            except OSError:
                pass


def thread(value):
    return {"threadId": value}


class CodexAdapterTests(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="cbc-cx-", dir="/tmp")
        self.grants = [
            {"context": "ctx-1", "member": "member-a", "device": "dev-1", "session": THREAD_A,
             "epoch": 1, "token": secrets.token_hex(16)},
            {"context": "ctx-1", "member": "member-b", "device": "dev-1", "session": THREAD_B,
             "epoch": 1, "token": secrets.token_hex(16)},
        ]
        self.core_path = os.path.join(self.root, "core.sock")
        self.core = FakeCore(self.core_path, self.grants, {ROOM: ("member-a", "member-b")})
        self.profile = os.path.join(self.root, "profile.json")
        write_private(self.profile, rust_json({"v": 1, "core_socket": self.core_path, "bindings": self.grants}))
        self.adapters = []

    def tearDown(self):
        for a in self.adapters:
            a.close()
        self.core.close()
        for name in sorted(os.listdir(self.root)):
            os.unlink(os.path.join(self.root, name))
        os.rmdir(self.root)

    def start(self, profile="default", extra_env=None):
        a = Adapter(self.profile if profile == "default" else profile, extra_env)
        self.adapters.append(a)
        a.reply(a.request("initialize", {"protocolVersion": "2025-06-18"}))
        return a

    def secrets_absent(self, adapter):
        out = b"".join(adapter.seen).decode()
        for g in self.grants:
            self.assertNotIn(g["token"], out)
        for reference in self.core.references:
            self.assertNotIn(reference, out)

    # -- tools

    def test_successful_receipt_nudges_only_configured_sibling_device_socket(self):
        path = os.path.join(self.root, "device.sock")
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(path)
        listener.listen(1)
        listener.settimeout(5)
        self.addCleanup(listener.close)
        received, done = [], threading.Event()
        def receive():
            try:
                conn, _ = listener.accept()
                with conn:
                    received.append(json.loads(read_line(conn)))
                    received.append(set(self.core.done.get("member-b", set())))
                    conn.sendall(b'{"ok":true}\n')
            finally:
                done.set()
        receiver = threading.Thread(target=receive, daemon=True)
        receiver.start()
        sender = self.start()
        reader = self.start(extra_env={"CBC_CANDIDATE_CONNECTOR": path})
        sender.call("cbc_send", {"room": ROOM, "body": "fixture", "key": "nudge"}, thread(THREAD_A))
        result, error = reader.call("cbc_check", {}, thread(THREAD_B))
        self.assertFalse(error)
        self.assertEqual(len(result["items"]), 1)
        self.assertTrue(done.wait(5))
        self.assertEqual(received[0], {"v": 1, "op": "codex_receipt", "auth": self.grants[1]})
        self.assertEqual(received[1], {1}, "nudge must follow an actual core receipt")
        self.secrets_absent(reader)

    def test_initialize_lists_bound_tools_and_check_takes_no_arguments(self):
        a = self.start()
        tools = a.reply(a.request("tools/list"))["result"]["tools"]
        self.assertEqual(sorted(t["name"] for t in tools), ["cbc_check", "cbc_connect", "cbc_discover", "cbc_send"])
        check = next(t for t in tools if t["name"] == "cbc_check")
        self.assertEqual(check["inputSchema"].get("properties"), {})
        self.assertFalse(check["inputSchema"].get("additionalProperties", True))

    def test_discover_and_connect_are_bound_without_receipting(self):
        a = self.start()
        result, error = a.call("cbc_discover", {}, thread(THREAD_A))
        self.assertFalse(error, result)
        self.assertTrue(result["members"])
        self.assertEqual(self.core.operations[-1], ("member-a", "discover", {"limit": 32}))
        arguments = {"target": "member-b", "kind": "peer", "scope_ref": "feature", "key": "connect-1"}
        result, error = a.call("cbc_connect", arguments, thread(THREAD_A))
        self.assertFalse(error, result)
        self.assertEqual(result["connection"]["room"], "connected-route")
        self.assertEqual(self.core.operations[-1], ("member-a", "connect", arguments | {"explanation": ""}))
        self.assertNotIn(("member-a", "receipt"), self.core.requests)
        self.secrets_absent(a)

    def test_discovery_and_connection_reject_model_identity_and_invalid_arguments(self):
        a = self.start()
        arguments = {"target": "member-b", "kind": "peer", "scope_ref": "feature", "key": "connect-1"}
        malformed = [("cbc_discover", {"member": "member-b"}), ("cbc_discover", []),
                     ("cbc_connect", arguments | {"session": THREAD_B}),
                     ("cbc_connect", arguments | {"kind": "admin"}),
                     ("cbc_connect", arguments | {"explanation": "x" * 601}),
                     ("cbc_connect", arguments | {"explanation": "😀" * 600}),
                     ("cbc_connect", arguments | {"kind": "report", "explanation": "override introduction"}),
                     ("cbc_connect", arguments | {"target": "\ud800"}),
                     ("cbc_connect", arguments | {"key": "\ncontrol"})]
        for name, value in malformed:
            with self.subTest(name=name, value=repr(value)[:100]):
                result, error = a.call(name, value, thread(THREAD_A))
                self.assertTrue(error, result)
        self.assertEqual(self.core.requests, [])
        result, error = a.call("cbc_discover", {}, None)
        self.assertTrue(error, result)
        self.assertEqual(self.core.requests, [])

    # -- binding by host metadata

    def target(self, **changes):
        value = {"v": 1, "profile": "unknown", "repository": "cbc-test",
                 "worktree": None, "cwd": os.path.realpath(os.getcwd()),
                 "saved_project": {"status": "not_exposed"},
                 "saved_environment": {"status": "not_exposed"},
                 "placement": {"evidence_ref": "synthetic-process-test"}}
        value.update(changes)
        return value

    def save_targets(self, target=None, authoritative=False, **changes):
        value = {"v": 2, "core_socket": self.core_path, "bindings": self.grants,
                 "targets": [{"session": grant["session"], "host": "codex",
                              "target": self.target() if target is None else target}
                             for grant in self.grants]}
        value.update(changes)
        with open(self.profile, "w") as stream:
            stream.write(rust_json(value))
        if authoritative:
            self.core.native_targets = {grant["member"]: json.loads(json.dumps(entry["target"]))
                                        for grant, entry in zip(value["bindings"], value["targets"])}
        return value

    def test_v2_unknown_profile_accepts_actual_cwd_and_receipts_normally(self):
        self.save_targets(self.target(worktree=os.path.dirname(os.path.realpath(os.getcwd()))), authoritative=True)
        a = self.start()
        result, error = a.call("cbc_send", {"room": ROOM, "body": "qualified", "key": "qualified"}, thread(THREAD_A))
        self.assertFalse(error, result)
        got, error = a.call("cbc_check", {}, thread(THREAD_B))
        self.assertFalse(error, got)
        self.assertEqual(got["items"][0]["body"], "qualified")
        self.assertTrue(self.core.wait_for("member-b", "receipt"))
        self.secrets_absent(a)

    def test_v2_target_constraints_fail_before_any_core_operation(self):
        a = self.start()
        mismatches = [
            self.target(cwd=os.path.realpath(self.root)),
            self.target(worktree=os.path.realpath(self.root)),
            self.target(profile="desktop"), self.target(profile="cli"),
            self.target(saved_project={"status": "configured", "id": "project-one"}),
            self.target(saved_environment={"status": "configured", "id": "environment-one"}),
        ]
        calls = [("cbc_check", {}), ("cbc_discover", {}),
                 ("cbc_send", {"room": ROOM, "body": "blocked", "key": "blocked"}),
                 ("cbc_connect", {"target": "member-b", "kind": "peer", "scope_ref": "feature", "key": "blocked"})]
        for target in mismatches:
            self.save_targets(target)
            for name, arguments in calls:
                with self.subTest(target=target, tool=name):
                    result, error = a.call(name, arguments, thread(THREAD_A))
                    self.assertTrue(error, result)
                    self.assertEqual(self.core.requests, [])
        self.secrets_absent(a)

    def test_v2_wrong_host_and_malformed_target_are_rejected(self):
        a = self.start()
        valid = self.save_targets(authoritative=True)
        for change in ("wrong-host", "missing-target", "extra-target-field", "extra-profile-field", "boolean-version"):
            value = json.loads(json.dumps(valid))
            if change == "wrong-host":
                value["targets"][0]["host"] = "claude"
            elif change == "missing-target":
                del value["targets"][0]["target"]
            elif change == "extra-target-field":
                value["targets"][0]["target"]["session"] = THREAD_A
            elif change == "extra-profile-field":
                value["arbitrary"] = "ignored-authority"
            else:
                value["v"] = True
            with open(self.profile, "w") as stream:
                stream.write(rust_json(value))
            with self.subTest(change=change):
                result, error = a.call("cbc_check", {}, thread(THREAD_A))
                self.assertTrue(error, result)
                self.assertEqual(self.core.requests, [])

    def test_claimed_profile_and_saved_ids_in_metadata_do_not_supply_observations(self):
        a = self.start()
        for target in [self.target(profile="desktop"),
                       self.target(saved_project={"status": "configured", "id": "project-one"})]:
            self.save_targets(target)
            claimed = {"threadId": THREAD_A, "profile": "desktop", "cwd": target["cwd"],
                       "saved_project": "project-one", "saved_environment": "environment-one"}
            result, error = a.call("cbc_check", {}, claimed)
            self.assertTrue(error, result)
            self.assertEqual(self.core.requests, [])

    def test_target_replacement_after_result_prevents_receipt_then_recovers(self):
        sender = self.start()
        sender.call("cbc_send", {"room": ROOM, "body": "retained", "key": "retained"}, thread(THREAD_A))
        valid = self.save_targets(authoritative=True)
        a = self.start()
        original = self.core.handle
        def replace_after_check(raw):
            reply = original(raw)
            if json.loads(raw)["request"]["op"] == "check":
                self.save_targets(self.target(cwd=os.path.realpath(self.root)))
            return reply
        self.core.handle = replace_after_check
        got, error = a.call("cbc_check", {}, thread(THREAD_B))
        self.assertFalse(error, got)
        self.assertEqual(got["items"][0]["body"], "retained")
        a.reply(a.request("ping"))  # Barrier after the adapter's receipt decision.
        self.assertNotIn(("member-b", "receipt"), self.core.requests)
        self.assertIn("member-b", self.core.pending)
        self.core.handle = original
        with open(self.profile, "w") as stream:
            stream.write(rust_json(valid))
        again, error = a.call("cbc_check", {}, thread(THREAD_B))
        self.assertFalse(error, again)
        self.assertEqual(again, got)
        self.assertTrue(self.core.wait_for("member-b", "receipt"))

    def test_v1_targets_remain_explicitly_unqualified_compatible(self):
        value = {"v": 1, "core_socket": self.core_path, "bindings": self.grants,
                 "targets": [{"session": grant["session"], "host": "codex"} for grant in self.grants]}
        with open(self.profile, "w") as stream:
            stream.write(rust_json(value))
        a = self.start()
        result, error = a.call("cbc_check", {}, thread(THREAD_A))
        self.assertFalse(error, result)
        self.assertEqual(result, {"items": []})

    def test_stored_target_rejects_local_tampering_and_v1_downgrade(self):
        self.save_targets(authoritative=True)
        a = self.start()
        values = [self.save_targets(self.target(repository="changed-local-repository")),
                  {"v": 1, "core_socket": self.core_path, "bindings": self.grants}]
        for value in values:
            with open(self.profile, "w") as stream:
                stream.write(rust_json(value))
            for name, args in [("cbc_check", {}), ("cbc_discover", {}),
                               ("cbc_send", {"room": ROOM, "body": "no", "key": "no"}),
                               ("cbc_connect", {"target": "member-b", "kind": "peer", "scope_ref": "scope", "key": "no"})]:
                result, error = a.call(name, args, thread(THREAD_A))
                self.assertTrue(error, result)
                self.assertEqual(self.core.requests, [])
        self.assertEqual(self.core.status_requests, ["member-a"] * 8)

    def test_missing_or_failed_authoritative_status_never_authorizes_legacy(self):
        a = self.start()
        for status in [{"type": "binding_status"}, {"type": "error", "message": "temporary"},
                       {"type": "empty"}, {"type": "binding_status", "native_target": {}}]:
            self.core.status_override = status
            result, error = a.call("cbc_check", {}, thread(THREAD_A))
            self.assertTrue(error, result)
            self.assertEqual(self.core.requests, [])
        self.core.status_override = {"type": "binding_status", "native_target": None}
        result, error = a.call("cbc_check", {}, thread(THREAD_A))
        self.assertFalse(error, result)
        self.assertEqual(self.core.requests, [("member-a", "check")])

    def test_authority_cache_is_bound_to_full_profile_and_only_success_is_cached(self):
        valid = self.save_targets(authoritative=True)
        a = self.start()
        for _ in range(3):
            result, error = a.call("cbc_check", {}, thread(THREAD_A))
            self.assertFalse(error, result)
        self.assertEqual(self.core.status_requests, ["member-a"])
        # Another member's audit metadata still changes the complete profile snapshot.
        valid["targets"][1]["target"]["placement"]["evidence_ref"] = "new-profile-audit"
        with open(self.profile, "w") as stream:
            stream.write(rust_json(valid))
        result, error = a.call("cbc_check", {}, thread(THREAD_A))
        self.assertFalse(error, result)
        self.assertEqual(self.core.status_requests, ["member-a", "member-a"])

    def test_full_profile_change_after_result_prevents_receipt_and_nudge(self):
        sender = self.start()
        sender.call("cbc_send", {"room": ROOM, "body": "snapshot", "key": "snapshot"}, thread(THREAD_A))
        valid = self.save_targets(authoritative=True)
        a = self.start()
        original = self.core.handle
        def change_snapshot(raw):
            reply = original(raw)
            if json.loads(raw)["request"]["op"] == "check":
                valid["targets"][0]["target"]["placement"]["evidence_ref"] = "revised-other-member"
                with open(self.profile, "w") as stream:
                    stream.write(rust_json(valid))
            return reply
        self.core.handle = change_snapshot
        result, error = a.call("cbc_check", {}, thread(THREAD_B))
        self.assertFalse(error, result)
        a.reply(a.request("ping"))
        self.assertNotIn(("member-b", "receipt"), self.core.requests)
        self.assertIn("member-b", self.core.pending)

    def test_each_call_acts_as_the_member_its_thread_is_bound_to(self):
        a = self.start()
        sent, err = a.call("cbc_send", {"room": ROOM, "key": "k1", "body": "hello beta"}, thread(THREAD_A))
        self.assertFalse(err, sent)
        self.assertEqual(sent, {"sequence": 1})
        empty, _ = a.call("cbc_check", meta=thread(THREAD_A))
        self.assertEqual(empty, {"items": []})
        got, err = a.call("cbc_check", meta=thread(THREAD_B))
        self.assertFalse(err, got)
        self.assertEqual([i["body"] for i in got["items"]], ["hello beta"])
        self.assertEqual([i["sender"] for i in got["items"]], ["member-a"])
        self.assertEqual(self.core.requests[:3], [("member-a", "send"), ("member-a", "check"), ("member-b", "check")])
        self.secrets_absent(a)

    def test_missing_or_unknown_thread_metadata_fails_closed_before_core(self):
        a = self.start()
        for meta in (None, {}, "thread-alpha", {"threadId": STRANGER}, {"threadId": ""}, {"threadId": 7},
                     {"thread_id": THREAD_A}, {"threadId": None, "thread_id": THREAD_A}):
            with self.subTest(meta=meta):
                for name, args in (("cbc_check", {}), ("cbc_send", {"room": ROOM, "key": "k", "body": "b"})):
                    out, err = a.call(name, args, meta)
                    self.assertTrue(err)
                    self.assertIn("unbound", out["error"])
        self.assertEqual(self.core.requests, [])

    def test_identity_in_arguments_is_refused_and_never_selects_a_member(self):
        a = self.start()
        spoofs = [("cbc_check", {"threadId": THREAD_B}),
                  ("cbc_check", {"_meta": {"threadId": THREAD_B}}),
                  ("cbc_send", {"room": ROOM, "key": "k", "body": "b", "member": "member-b"}),
                  ("cbc_send", {"room": ROOM, "key": "k", "body": "b", "threadId": THREAD_B}),
                  ("cbc_send", {"room": ROOM, "key": "k", "body": "b", "auth": self.grants[1]})]
        for name, args in spoofs:
            with self.subTest(name=name, args=sorted(args)):
                out, err = a.call(name, args, thread(THREAD_A))
                self.assertTrue(err)
        self.assertEqual(self.core.requests, [])
        self.secrets_absent(a)

    # -- receipts

    def test_batch_is_receipted_only_after_its_result_is_written(self):
        a = self.start()
        a.call("cbc_send", {"room": ROOM, "key": "k1", "body": "one"}, thread(THREAD_A))
        got, _ = a.call("cbc_check", meta=thread(THREAD_B))
        self.assertEqual(len(got["items"]), 1)
        self.assertNotIn("reference", got)
        self.assertTrue(self.core.wait_for("member-b", "receipt"))
        again, _ = a.call("cbc_check", meta=thread(THREAD_B))
        self.assertEqual(again, {"items": []})
        self.secrets_absent(a)

    def test_failed_result_write_is_not_receipted_and_the_batch_is_redelivered(self):
        a = self.start()
        a.call("cbc_send", {"room": ROOM, "key": "k1", "body": "keep me"}, thread(THREAD_A))
        gone = Adapter(self.profile, host_gone=True)
        self.adapters.append(gone)
        gone.request("tools/call", {"name": "cbc_check", "arguments": {}, "_meta": thread(THREAD_B)})
        self.assertTrue(self.core.wait_for("member-b", "check"))
        time.sleep(0.5)
        self.assertNotIn(("member-b", "receipt"), self.core.requests)
        b = self.start()
        got, err = b.call("cbc_check", meta=thread(THREAD_B))
        self.assertFalse(err, got)
        self.assertEqual([i["body"] for i in got["items"]], ["keep me"])

    # -- send bounds, as the Claude adapter enforces them

    def test_send_arguments_are_enforced_not_just_declared(self):
        a = self.start()
        for args in ({"room": ROOM, "key": "k"}, {"room": ROOM, "key": "k", "body": ""},
                     {"room": ROOM, "key": "k", "body": 5}, {"room": "r" * 129, "key": "k", "body": "b"},
                     {"room": ROOM, "key": "k" * 129, "body": "b"}, {"room": ROOM, "key": "k", "body": "é" * 8193},
                     {"room": ROOM, "key": "k", "body": "\ud800"}):
            with self.subTest(args=sorted(args)):
                out, err = a.call("cbc_send", args, thread(THREAD_A))
                self.assertTrue(err)
        self.assertEqual(self.core.requests, [])

    def test_body_at_the_escaped_limit_round_trips(self):
        a = self.start()
        body = '"\\' * 8192
        self.assertEqual(len(rust_json(body).encode()) - 2, MAX_ESCAPED_BODY)
        sent, err = a.call("cbc_send", {"room": ROOM, "key": "k", "body": body}, thread(THREAD_A))
        self.assertFalse(err, sent)
        got, _ = a.call("cbc_check", meta=thread(THREAD_B))
        self.assertEqual(got["items"][0]["body"], body)

    def test_body_over_the_escaped_limit_is_refused_before_core(self):
        a = self.start()
        for body in ("\x01" + '"' * 16382, "\x01" * 16384):
            out, err = a.call("cbc_send", {"room": ROOM, "key": "k", "body": body}, thread(THREAD_A))
            self.assertTrue(err)
            self.assertIn("escaped", out["error"])
        self.assertEqual(self.core.requests, [])

    def test_ansi_and_control_text_is_ordinary_text(self):
        a = self.start()
        body = "\x1b[31mred\x1b[0m\ttab\r\nline\x00nul sep"
        a.call("cbc_send", {"room": ROOM, "key": "k", "body": body}, thread(THREAD_A))
        got, _ = a.call("cbc_check", meta=thread(THREAD_B))
        self.assertEqual(got["items"][0]["body"], body)

    # -- framing and failure

    def test_oversized_input_frame_is_refused_and_adapter_keeps_serving(self):
        a = self.start()
        a.send_raw(b'{"jsonrpc":"2.0","id":99,"method":"ping","params":{"pad":"' + b"x" * (300 * 1024) + b'"}}\n')
        err = json.loads(a.lines.get(timeout=5))
        self.assertEqual(err["error"]["code"], -32600)
        out, is_err = a.call("cbc_check", meta=thread(THREAD_B))
        self.assertFalse(is_err, out)

    def test_stdio_is_utf8_whatever_the_locale(self):
        a = self.start(extra_env={"LANG": "C", "LC_ALL": "C", "PYTHONIOENCODING": "ascii"})
        a.call("cbc_send", {"room": ROOM, "key": "k", "body": "naïve 🚀"}, thread(THREAD_A))
        got, _ = a.call("cbc_check", meta=thread(THREAD_B))
        self.assertEqual(got["items"][0]["body"], "naïve 🚀")

    def test_unavailable_core_is_an_error_result_not_a_crash(self):
        self.core.close()
        os.unlink(self.core_path)
        a = self.start()
        out, err = a.call("cbc_check", meta=thread(THREAD_A))
        self.assertTrue(err)
        self.assertIn("core unavailable", out["error"])
        self.assertIsNone(a.proc.poll())
        self.secrets_absent(a)
        open(self.core_path, "w").close()  # tearDown unlinks it

    # -- profile: required by name at startup, read on each call

    def test_valid_profile_over_64k_loads_but_128k_limit_is_enforced(self):
        with open(self.profile, "a") as stream:
            stream.write(" " * (70 * 1024))
        a = self.start()
        out, error = a.call("cbc_check", meta=thread(THREAD_A))
        self.assertFalse(error)
        self.assertEqual(out, {"items": []})
        with open(self.profile, "a") as stream:
            stream.write(" " * (60 * 1024))
        out, error = a.call("cbc_check", meta=thread(THREAD_A))
        self.assertTrue(error)
        self.assertIn("profile too large", out["error"])

    def test_fifo_profile_is_rejected_without_waiting_for_a_writer(self):
        path = os.path.join(self.root, "fifo-profile")
        os.mkfifo(path, 0o600)
        a = self.start(profile=path)
        out, error = a.call("cbc_check", meta=thread(THREAD_A))
        self.assertTrue(error)
        self.assertIn("regular file", out["error"])

    def test_profile_path_must_be_explicit(self):
        a = Adapter(None)
        self.adapters.append(a)
        self.assertNotEqual(a.proc.wait(timeout=5), 0)
        self.assertIn("CBC_CANDIDATE_PROFILE", a.proc.stderr.read().decode())

    def test_profile_written_after_startup_is_used_on_the_next_call(self):
        later = os.path.join(self.root, "later.json")
        a = self.start(later)
        tools = a.reply(a.request("tools/list"))["result"]["tools"]
        self.assertEqual({tool["name"] for tool in tools},
                         {"cbc_check", "cbc_connect", "cbc_discover", "cbc_send"})
        out, err = a.call("cbc_check", meta=thread(THREAD_B))
        self.assertTrue(err)
        self.assertIn("profile", out["error"])
        self.assertEqual(self.core.requests, [])
        write_private(later, rust_json({"v": 1, "core_socket": self.core_path, "bindings": self.grants[1:]}))
        out, err = a.call("cbc_check", meta=thread(THREAD_B))
        self.assertFalse(err, out)
        self.assertEqual(self.core.requests, [("member-b", "check")])
        out, err = a.call("cbc_check", meta=thread(THREAD_A))  # not enrolled by the later profile
        self.assertTrue(err)
        self.assertIn("unbound", out["error"])
        self.assertEqual(len(self.core.requests), 1)

    def test_unsafe_profiles_are_refused_per_call_and_the_adapter_keeps_serving(self):
        data = rust_json({"v": 1, "core_socket": self.core_path, "bindings": self.grants})
        loose = os.path.join(self.root, "loose.json")
        write_private(loose, data, 0o644)
        link = os.path.join(self.root, "link.json")
        os.symlink(self.profile, link)
        hard = os.path.join(self.root, "hard.json")
        os.link(self.profile, hard)
        big = os.path.join(self.root, "big.json")
        write_private(big, " " * (128 * 1024 + 1))
        dup = os.path.join(self.root, "dup.json")
        write_private(dup, rust_json({"v": 1, "core_socket": self.core_path, "bindings": self.grants + self.grants[:1]}))
        extra = os.path.join(self.root, "extra.json")
        write_private(extra, rust_json({"v": 1, "core_socket": self.core_path,
                                        "bindings": [dict(self.grants[0], note="x")]}))
        garbage = os.path.join(self.root, "garbage.json")
        write_private(garbage, "[1, 2")
        for path in (loose, link, hard, big, dup, extra, garbage, os.path.join(self.root, "missing.json")):
            with self.subTest(path=os.path.basename(path)):
                a = self.start(path)
                out, err = a.call("cbc_check", meta=thread(THREAD_A))
                self.assertTrue(err)
                self.assertIn("profile", out["error"])
                self.assertIsNone(a.proc.poll())
                self.secrets_absent(a)
        self.assertEqual(self.core.requests, [])
        os.unlink(hard)  # give the main profile back its single link


if __name__ == "__main__":
    unittest.main()
