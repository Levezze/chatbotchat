#!/usr/bin/env python3
"""Process-level tests: adapter -> router -> fake core, with real peer processes.

Each member is a real "fake Claude" parent process that spawns the adapter, so the
router's peer check (socket uid, peer pid, parent pid, parent session record,
kernel start time) runs for real. The core is a fake that speaks the Rust v1 wire
and rejects unknown fields. These tests say nothing about Claude Code delivery.
Run: python3 -I test_router.py -v
"""
import importlib.util, json, os, queue, secrets, select, shutil, signal, socket, subprocess, sys, tempfile, threading, time, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ALPHA, BETA, GAMMA, STRANGER = "sess-alpha", "sess-beta", "sess-gamma", "sess-stranger"

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("router", os.path.join(HERE, "router.py"))
router = importlib.util.module_from_spec(spec)
spec.loader.exec_module(router)
spec = importlib.util.spec_from_file_location("adapter", os.path.join(HERE, "adapter.py"))
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)
MAX_BATCH_BYTES = 48 * 1024  # inbox.rs: budget for a new batch
LEGACY_BATCH_BYTES = 128 * 1024  # batches persisted under the old budget may still be pending
MAX_ESCAPED_BODY = 32 * 1024  # serde_json-escaped body, without its two quotes


def rust_json(value):
    """serde_json's compact encoding: UTF-8 kept, no spaces."""
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def read_text(path):
    with open(path) as f:
        return f.read()


def utc_lstart(pid):
    out = subprocess.run(["ps", "-o", "lstart=", "-p", str(pid)], capture_output=True, text=True,
                         env={"TZ": "UTC", "PATH": "/bin:/usr/bin"}).stdout
    return " ".join(out.split())


def group_alive(pgid):
    try:
        os.killpg(pgid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


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
    OPS = {"check": set(), "send": {"room", "key", "body"}, "receipt": {"reference"}, "close": {"room"},
           "binding_status": set(),
           "watch": {"others", "after"}, "watch_members": {"others", "after"},
           "discover": {"limit"}, "connect": {"target", "kind", "scope_ref", "key", "explanation"}}
    AUTH = {"context", "member", "device", "session", "epoch", "token"}

    def __init__(self, path, grants, rooms):
        self.tokens = {g["member"]: g["token"] for g in grants}
        self.rooms = rooms
        self.items, self.pending, self.done, self.keys, self.requests = [], {}, {}, {}, []
        self.operations = []
        self.revoked = set()
        self.watches, self.watch_error = [], False  # one (members, after) per watch connection
        self.budget = MAX_BATCH_BYTES
        self.feed = threading.Condition()
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
            threading.Thread(target=self._serve, args=(conn,), daemon=True).start()

    def _serve(self, conn):
        with conn:
            try:
                conn.sendall((rust_json(self.handle(read_line(conn), conn)) + "\n").encode())
            except OSError:
                pass

    def close(self):
        self.srv.close()

    def inject(self, member, room, body):
        """A send that reaches the core without passing the router, as a Codex adapter's does."""
        with self.feed:
            item = {"sequence": len(self.items) + 1, "room": room, "sender": member, "kind": "message", "body": body}
            self.items.append(item)
            self.feed.notify_all()
        return item["sequence"]

    def revision(self, members):
        """inbox.rs: the newest unreceipted item any of these members may read but did not send."""
        return max((i["sequence"] for i in self.items for m in members
                    if i["sender"] != m and m in self.rooms[i["room"]]
                    and i["sequence"] not in self.done.get(m, set())), default=0)

    def watch(self, auth, others, after, conn, targeted=False):
        if not isinstance(others, list) or len(others) >= 64 or not isinstance(after, int) or after < 0:
            return {"type": "error", "message": "candidate operation rejected"}
        members = [auth["member"]]
        for other in others:
            if not isinstance(other, dict) or set(other) != self.AUTH or self.tokens.get(other["member"]) != other["token"]:
                return {"type": "error", "message": "candidate operation rejected"}
            members.append(other["member"])
        self.watches.append((tuple(members), after))
        if self.watch_error:
            return {"type": "error", "message": "candidate operation rejected"}
        with self.feed:
            while (sequence := self.revision(members)) <= after:
                self.feed.wait(0.05)
                readable = select.select([conn], [], [], 0)[0]
                if readable and not conn.recv(1, socket.MSG_PEEK):
                    raise OSError("watch client closed")
        if targeted:
            changes = [{"context": auth["context"], "member": member, "sequence": newest}
                       for member in members if (newest := self.revision([member])) > after]
            return {"type": "members_changed", "sequence": sequence, "members": changes}
        return {"type": "changed", "sequence": sequence}

    def handle(self, raw, conn=None):
        try:
            req = json.loads(raw)
            assert set(req) == {"v", "auth", "request"} and req["v"] == 1
            auth, op = req["auth"], dict(req["request"])
            assert set(auth) == self.AUTH
            name = op.pop("op")
            assert set(op) == self.OPS[name]
        except (ValueError, AssertionError, KeyError, TypeError):
            return {"type": "error", "message": "invalid candidate request"}
        member = auth["member"]
        if self.tokens.get(member) != auth["token"]:
            return {"type": "error", "message": "candidate operation rejected"}
        if name == "binding_status":
            if member in self.revoked:
                return {"type": "error", "code": "invalid_binding", "message": "revoked"}
            return {"type": "binding_status", "native_target": None,
                    "first_unread_sequence": None, "last_receipted_sequence": 0}
        if name in ("watch", "watch_members"):
            return self.watch(auth, op["others"], op["after"], conn, targeted=name == "watch_members")
        self.requests.append((member, name))
        with self.feed:
            reply = self.dispatch(member, name, op)
            if reply["type"] == "sent":
                self.feed.notify_all()
        return reply

    def dispatch(self, member, name, op):
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
                batch = []
                for i in items[:32]:  # inbox.rs: stop before the items exceed the byte budget
                    if len(rust_json(batch + [i]).encode()) > self.budget:
                        break
                    batch.append(i)
                self.pending[member] = {"reference": secrets.token_hex(32), "items": batch}
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


class FakeInbox:
    def __init__(self, path):
        self.lines = []
        self.srv = socket.socket(socket.AF_UNIX)
        self.srv.bind(path)
        self.srv.listen(4)
        threading.Thread(target=self._run, daemon=True).start()

    def _run(self):
        while True:
            try:
                conn, _ = self.srv.accept()
            except OSError:
                return
            with conn:
                self.lines.append(conn.recv(65536).decode())

    def close(self):
        self.srv.close()


# A stand-in for the Claude Code process: waits for the test to write (or not write)
# its session record, then runs the adapter with inherited stdio.
PARENT = ("import os,subprocess,sys,time\n"
          "go=sys.argv[1]\n"
          "while not os.path.exists(go): time.sleep(0.02)\n"
          "sys.exit(subprocess.call(sys.argv[2:]))\n")


class RouterTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="cbcr-", dir="/tmp")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.sessions = os.path.join(self.tmp, "sessions")
        os.makedirs(self.sessions)
        self.grants = [{"context": "ctx", "member": m, "device": "dev1", "session": s, "epoch": 1,
                        "token": "cbcc1_" + secrets.token_hex(32)} for m, s in (("alpha", ALPHA), ("beta", BETA), ("gamma", GAMMA))]
        self.core = FakeCore(os.path.join(self.tmp, "core.sock"), self.grants, {"t1": {"alpha", "beta"}})
        self.addCleanup(self.core.close)
        self.profile = os.path.join(self.tmp, "profile.json")
        fd = os.open(self.profile, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump({"v": 1, "core_socket": os.path.join(self.tmp, "core.sock"), "bindings": self.grants}, f)
        self.inboxes = {}
        self.router_sock = os.path.join(self.tmp, "router.sock")
        self.router = subprocess.Popen(
            [sys.executable, "-I", os.path.join(HERE, "router.py"), "--profile", self.profile,
             "--listen", self.router_sock,
             "--sessions-dir", self.sessions, "--log-dir", self.tmp, "--ttl", "60"])
        self.addCleanup(self.stop_router)  # before the start check: a failed setUp skips tearDown
        self.assertTrue(self.wait_for(lambda: os.path.exists(self.router_sock), secs=15), "router did not start")

    def stop_router(self):
        self.router.terminate()
        self.router.wait()

    def test_restart_with_one_revoked_binding_still_serves_the_others(self):
        self.stop_router()
        if os.path.exists(self.router_sock):  # existence must prove the new router
            os.unlink(self.router_sock)
        self.core.revoked.add("gamma")
        self.router = subprocess.Popen(
            [sys.executable, "-I", os.path.join(HERE, "router.py"), "--profile", self.profile,
             "--listen", self.router_sock,
             "--sessions-dir", self.sessions, "--log-dir", self.tmp, "--ttl", "60"])
        self.assertTrue(self.wait_for(lambda: os.path.exists(self.router_sock), secs=15),
                        "one revoked binding aborted the router for every chat")
        log = os.path.join(self.tmp, "router-events.jsonl")
        self.assertTrue(self.wait_for(lambda: os.path.exists(log) and any(
            json.loads(line).get("event") == "bindings_quarantined" and json.loads(line).get("count") == 1
            for line in open(log)), secs=5), "revoked binding was not reported as quarantined")

    def inbox(self, name):
        if name not in self.inboxes:
            self.inboxes[name] = FakeInbox(os.path.join(self.tmp, f"in-{name}.sock"))
            self.addCleanup(self.inboxes[name].close)
        return self.inboxes[name]

    def spawn(self, argv, env):
        """Start a test-owned process group and register its full teardown."""
        p = subprocess.Popen(argv, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             text=True, encoding="utf-8", start_new_session=True)
        p.lines = queue.Queue()
        def pump():
            for line in p.stdout:
                p.lines.put(line)
            p.lines.put("")
        pump_thread = threading.Thread(target=pump, daemon=True)
        pump_thread.start()
        def reap():
            p.stdin.close()
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            p.wait()
            pump_thread.join(5)
            p.stdout.close()
            gone = self.wait_for(lambda: not group_alive(p.pid))
            self.assertTrue(gone, "test process group survived teardown")
        self.addCleanup(reap)
        return p

    def live_record(self, session, inbox):
        """A session record for a live process other than a member, for ringing only."""
        with open(os.path.join(self.sessions, f"{os.getpid()}.json"), "w") as f:
            json.dump({"pid": os.getpid(), "sessionId": session, "messagingSocketPath": inbox}, f)

    def member(self, session, record="ok", inbox=None, extra_env=None):
        """Start a fake Claude parent and its adapter. record: ok | none | badstart | badpid."""
        go = os.path.join(self.tmp, f"go-{secrets.token_hex(4)}")
        env = {**os.environ, "CBC_CANDIDATE_CONNECTOR": self.router_sock, "CLAUDE_CODE_SESSION_ID": session,
               **(extra_env or {})}
        p = self.spawn([sys.executable, "-I", "-c", PARENT, go, sys.executable, "-I",
                        os.path.join(HERE, "adapter.py")], env)
        if record != "none":
            rec = {"pid": p.pid, "sessionId": session, "procStart": utc_lstart(p.pid),
                   "messagingSocketPath": inbox or os.path.join(self.tmp, f"in-{session}.sock")}
            if record == "badstart":
                rec["procStart"] = "Thu Jan  1 00:00:00 2015"
            if record == "badpid":
                rec["pid"] = p.pid + 100000
            with open(os.path.join(self.sessions, f"{p.pid}.json"), "w") as f:
                json.dump(rec, f)
        open(go, "w").close()
        return p

    def rpc_raw(self, p, rid, method, params=None):
        p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}}) + "\n")
        p.stdin.flush()
        return self.readline(p)

    def readline(self, p, secs=10):
        """Next stdout line, or a test failure instead of a hang if the adapter stalls."""
        try:
            line = p.lines.get(timeout=secs)
        except queue.Empty:
            self.fail("adapter wrote no complete frame")
        self.assertTrue(line, "adapter closed stdout")
        return line

    def rpc(self, p, rid, method, params=None):
        return json.loads(self.rpc_raw(p, rid, method, params))

    def tool(self, p, rid, name, args=None):
        res = self.rpc(p, rid, "tools/call", {"name": name, "arguments": args or {}})
        return json.loads(res["result"]["content"][0]["text"]), res["result"]["isError"]

    def wait_for(self, cond, secs=10.0):  # generous: core builds share this machine
        end = time.time() + secs
        while time.time() < end and not cond():
            time.sleep(0.05)
        return cond()

    def events(self):
        path = os.path.join(self.tmp, "router-events.jsonl")
        if not os.path.exists(path):
            return []
        return [json.loads(l) for l in read_text(path).splitlines()]

    # --- delivery ---------------------------------------------------------------

    def test_send_rings_other_member_and_check_receipts_through_core(self):
        alpha_inbox = self.inbox("alpha")
        alpha = self.member(ALPHA, inbox=os.path.join(self.tmp, "in-alpha.sock"))
        beta = self.member(BETA)
        out, is_error = self.tool(beta, 1, "cbc_send", {"room": "t1", "body": "hello alpha", "key": "k1"})
        self.assertFalse(is_error, out)
        self.assertEqual(out, {"sequence": 1})
        self.assertTrue(self.wait_for(lambda: alpha_inbox.lines))
        self.assertNotIn("hello alpha", alpha_inbox.lines[0])
        payload, _ = self.tool(alpha, 1, "cbc_check")
        self.assertEqual([i["body"] for i in payload["items"]], ["hello alpha"])
        self.assertNotIn("reference", payload)
        self.assertTrue(self.wait_for(lambda: ("alpha", "receipt") in self.core.requests))
        payload, _ = self.tool(alpha, 2, "cbc_check")
        self.assertEqual(payload["items"], [])

    # --- the shared core feed -----------------------------------------------------

    def restart_router(self):
        self.router.terminate()
        self.router.wait()
        self.router = subprocess.Popen(
            [sys.executable, "-I", os.path.join(HERE, "router.py"), "--profile", self.profile,
             "--listen", self.router_sock,
             "--sessions-dir", self.sessions, "--log-dir", self.tmp, "--ttl", "60"])
        self.assertTrue(self.wait_for(lambda: os.path.exists(self.router_sock), secs=15), "router did not restart")

    def test_send_that_bypasses_the_router_still_rings_through_the_core_feed(self):
        alpha_inbox = self.inbox("alpha")
        self.member(ALPHA, inbox=os.path.join(self.tmp, "in-alpha.sock"))
        self.assertTrue(self.wait_for(lambda: ("alpha", "check") in self.core.requests))  # startup hello
        self.core.inject("beta", "t1", "from another harness")
        self.assertTrue(self.wait_for(lambda: alpha_inbox.lines), "a core-side send must ring the reader")
        self.assertNotIn("from another harness", alpha_inbox.lines[0])

    def test_shared_wake_never_checks_an_unaffected_live_chat(self):
        alpha_inbox = self.inbox("alpha")
        self.member(ALPHA, inbox=os.path.join(self.tmp, "in-alpha.sock"))
        self.assertTrue(self.wait_for(lambda: ("alpha", "check") in self.core.requests))
        self.inbox("gamma")
        self.live_record(GAMMA, os.path.join(self.tmp, "in-gamma.sock"))
        self.core.inject("beta", "t1", "target alpha only")
        self.assertTrue(self.wait_for(lambda: alpha_inbox.lines))
        self.assertIn(("alpha", "check"), self.core.requests)
        self.assertNotIn(("gamma", "check"), self.core.requests)

    def test_unavailable_chat_is_retried_without_a_new_send_or_hello(self):
        self.core.inject("beta", "t1", "arrived before the chat process")
        self.assertTrue(self.wait_for(lambda: any(e["event"] == "target_failed" for e in self.events())))
        inbox = self.inbox("alpha")
        self.live_record(ALPHA, os.path.join(self.tmp, "in-alpha.sock"))
        self.assertTrue(self.wait_for(lambda: inbox.lines, secs=4),
                        "pending delivery waited for another message or adapter hello")
        self.assertNotIn("arrived before", inbox.lines[0])

    def test_opt_in_channel_delivers_content_free_mcp_events_without_a_messaging_socket(self):
        alpha = self.member(ALPHA, extra_env={"CBC_CANDIDATE_CHANNEL": "1"})
        response = self.rpc(alpha, 1, "initialize", {"protocolVersion": "2025-06-18"})
        experimental = response["result"]["capabilities"]["experimental"]
        self.assertEqual(experimental, {"claude/channel": {}})
        self.assertNotIn("claude/channel/permission", experimental)
        alpha.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        alpha.stdin.flush()
        self.assertTrue(self.wait_for(lambda: ("alpha", "check") in self.core.requests))
        self.core.inject("beta", "t1", "private body must be fetched, not pushed")
        event = json.loads(self.readline(alpha))
        self.assertEqual(event["method"], "notifications/claude/channel")
        self.assertEqual(event["params"]["meta"], {"kind": "cbc_pending"})
        self.assertNotIn("private body", str(event))
        self.assertNotIn("reference", str(event))
        self.assertEqual(self.core.done.get("alpha", set()), set(), "a channel hint receipted the body")
        payload, error = self.tool(alpha, 2, "cbc_check")
        self.assertFalse(error)
        self.assertEqual(payload["items"][0]["body"], "private body must be fetched, not pushed")
        self.assertTrue(self.wait_for(lambda: bool(self.core.done.get("alpha"))))
        self.assertTrue(any(e["event"] == "channel_written" for e in self.events()))
        self.assertFalse(any(e["event"] == "doorbell_written" for e in self.events()))

    def test_channel_reconnect_reoffers_unread_items_without_another_send(self):
        alpha = self.member(ALPHA, extra_env={"CBC_CANDIDATE_CHANNEL": "1"})
        self.rpc(alpha, 1, "initialize", {"protocolVersion": "2025-06-18"})
        alpha.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        alpha.stdin.flush()
        self.assertTrue(self.wait_for(lambda: ("alpha", "check") in self.core.requests))
        self.core.inject("beta", "t1", "still unread")
        self.assertEqual(json.loads(self.readline(alpha))["method"], "notifications/claude/channel")
        self.restart_router()
        self.assertEqual(json.loads(self.readline(alpha))["method"], "notifications/claude/channel")
        self.assertEqual(self.core.done.get("alpha", set()), set())
        payload, error = self.tool(alpha, 2, "cbc_check")
        self.assertFalse(error)
        self.assertEqual(payload["items"][0]["body"], "still unread")

    def test_router_holds_one_feed_for_every_binding_and_reopens_it_only_after_a_change(self):
        self.assertTrue(self.wait_for(lambda: len(self.core.watches) == 1), "router opened no feed")
        time.sleep(1.0)
        self.assertEqual(len(self.core.watches), 1, "an idle feed must stay parked, not poll")
        members, after = self.core.watches[0]
        self.assertEqual(sorted(members), ["alpha", "beta", "gamma"])
        self.assertEqual(after, 0)
        sequence = self.core.inject("beta", "t1", "wake")
        self.assertTrue(self.wait_for(lambda: len(self.core.watches) == 2))
        self.assertEqual(self.core.watches[1][1], sequence)
        time.sleep(0.5)
        self.assertEqual(len(self.core.watches), 2)

    def test_restarted_router_rings_for_items_left_unread_while_it_was_down(self):
        alpha_inbox = self.inbox("alpha")
        self.live_record(ALPHA, os.path.join(self.tmp, "in-alpha.sock"))  # alive, but no adapter hello
        self.router.terminate()
        self.router.wait()
        self.core.inject("beta", "t1", "while the router was down")
        self.restart_router()
        self.assertTrue(self.wait_for(lambda: alpha_inbox.lines), "a fresh feed starts at 0 and recovers")

    def test_failing_feed_backs_off_instead_of_spinning(self):
        self.core.watch_error = True
        self.restart_router()
        time.sleep(2.5)
        self.assertLessEqual(len(self.core.watches), 4, "router retried a failing feed in a tight loop")
        self.core.watch_error = False
        alpha_inbox = self.inbox("alpha")
        self.live_record(ALPHA, os.path.join(self.tmp, "in-alpha.sock"))
        self.core.inject("beta", "t1", "after recovery")
        self.assertTrue(self.wait_for(lambda: alpha_inbox.lines, secs=6), "feed did not recover")

    def test_relaunched_member_is_rung_for_items_sent_while_it_was_gone(self):
        beta = self.member(BETA)
        self.tool(beta, 1, "cbc_send", {"room": "t1", "body": "while away", "key": "k1"})
        live = self.inbox("alpha")
        self.member(ALPHA, inbox=os.path.join(self.tmp, "in-alpha.sock"))
        self.assertTrue(self.wait_for(lambda: live.lines), "hello did not ring the relaunched member")
        self.assertNotIn("while away", live.lines[0])

    def test_send_rings_only_members_with_pending_items(self):
        alpha_inbox, gamma_inbox = self.inbox("alpha"), self.inbox("gamma")
        self.live_record(GAMMA, os.path.join(self.tmp, "in-gamma.sock"))
        self.member(ALPHA, inbox=os.path.join(self.tmp, "in-alpha.sock"))
        beta = self.member(BETA)
        self.tool(beta, 1, "cbc_send", {"room": "t1", "body": "for t1 only", "key": "k1"})
        self.assertTrue(self.wait_for(lambda: alpha_inbox.lines))
        time.sleep(0.3)
        self.assertEqual(gamma_inbox.lines, [], "gamma is bound but not in t1")

    def test_repeated_sends_ring_once_per_pending_batch_and_hello_rerings(self):
        alpha_inbox = self.inbox("alpha")
        self.member(ALPHA, inbox=os.path.join(self.tmp, "in-alpha.sock"))
        beta = self.member(BETA)
        for n in range(3):
            self.tool(beta, n + 1, "cbc_send", {"room": "t1", "body": f"m{n}", "key": f"k{n}"})
        self.assertTrue(self.wait_for(lambda: alpha_inbox.lines))
        time.sleep(0.3)
        self.assertEqual(len(alpha_inbox.lines), 1, "one bell per pending batch")
        self.member(ALPHA, inbox=os.path.join(self.tmp, "in-alpha.sock"))  # relaunch: hello
        self.assertTrue(self.wait_for(lambda: len(alpha_inbox.lines) == 2), "hello must re-ring")

    def test_receipt_rerings_while_items_remain_so_none_strand(self):
        alpha_inbox = self.inbox("alpha")
        alpha = self.member(ALPHA, inbox=os.path.join(self.tmp, "in-alpha.sock"))
        beta = self.member(BETA)
        for n in range(33):
            self.tool(beta, n + 1, "cbc_send", {"room": "t1", "body": f"m{n}", "key": f"k{n}"})
        self.assertTrue(self.wait_for(lambda: alpha_inbox.lines))
        got, rid = [], 1
        while True:
            bells = len(alpha_inbox.lines)
            payload, _ = self.tool(alpha, rid, "cbc_check")
            rid += 1
            if not payload["items"]:
                break
            got += [i["body"] for i in payload["items"]]
            receipts = self.core.requests.count(("alpha", "receipt"))
            self.assertTrue(self.wait_for(lambda: self.core.requests.count(("alpha", "receipt")) == receipts))
            if len(got) < 33:
                self.assertTrue(self.wait_for(lambda: len(alpha_inbox.lines) > bells),
                                f"no re-ring after receipt with {33 - len(got)} items left")
            self.assertLess(rid, 40)
        self.assertEqual(got, [f"m{n}" for n in range(33)])
        time.sleep(0.3)
        self.assertEqual(len(alpha_inbox.lines), bells, "no bell once the inbox is drained")

    def test_failed_receipt_rerings_so_the_redelivery_is_not_stranded(self):
        alpha_inbox = self.inbox("alpha")
        alpha = self.member(ALPHA, inbox=os.path.join(self.tmp, "in-alpha.sock"))
        beta = self.member(BETA)
        self.tool(beta, 1, "cbc_send", {"room": "t1", "body": "again", "key": "k1"})
        self.assertTrue(self.wait_for(lambda: alpha_inbox.lines))
        real = self.core.handle
        def cancel_before_receipt(raw, conn=None):  # the core cancels the batch, e.g. a superseded epoch
            if b'"op":"receipt"' in raw.replace(b" ", b""):
                self.core.pending["alpha"]["reference"] = secrets.token_hex(32)
            return real(raw, conn)
        self.core.handle = cancel_before_receipt
        payload, _ = self.tool(alpha, 1, "cbc_check")
        self.assertEqual([i["body"] for i in payload["items"]], ["again"])
        self.assertTrue(self.wait_for(lambda: len(alpha_inbox.lines) == 2),
                        "items still pending after a failed receipt must ring again")

    def test_hello_reports_an_unavailable_core_instead_of_ok(self):
        self.core.close()
        os.unlink(os.path.join(self.tmp, "core.sock"))
        p = self.member(ALPHA)
        self.assertTrue(self.wait_for(lambda: any(e["event"] == "hello" for e in self.events())))
        logs = [json.loads(l) for n in os.listdir(self.tmp) if n.startswith("adapter-")
                for l in read_text(os.path.join(self.tmp, n)).splitlines()]
        hello = [l for l in logs if l.get("event") == "hello"]
        self.assertTrue(self.wait_for(lambda: hello), "adapter did not log hello")
        self.assertIsNone(hello[0]["ok"])
        self.assertIn("core", hello[0]["error"])
        self.assertEqual([e.get("reply") for e in self.events() if e["event"] == "hello"], ["error"])

    def test_router_exits_at_ttl_and_removes_its_socket(self):
        path = os.path.join(self.tmp, "ttl.sock")
        r = subprocess.Popen([sys.executable, "-I", os.path.join(HERE, "router.py"), "--profile",
                              self.profile, "--listen", path, "--sessions-dir", self.sessions,
                              "--ttl", "1"])
        try:
            self.assertEqual(r.wait(timeout=4), 0)
        finally:
            r.kill()
            r.wait()
        self.assertFalse(os.path.lexists(path))

    def test_only_profile_sessions_are_ever_rung(self):
        stranger_inbox = self.inbox("stranger")
        with open(os.path.join(self.sessions, "99999.json"), "w") as f:
            json.dump({"pid": 99999, "sessionId": STRANGER,
                       "messagingSocketPath": os.path.join(self.tmp, "in-stranger.sock")}, f)
        beta = self.member(BETA)
        self.tool(beta, 1, "cbc_send", {"room": "t1", "body": "x", "key": "k1"})
        time.sleep(0.4)
        self.assertEqual(stranger_inbox.lines, [])

    # --- binding boundary --------------------------------------------------------

    def test_member_without_a_session_record_is_unbound(self):
        p = self.member(ALPHA, record="none")
        payload, is_error = self.tool(p, 1, "cbc_check")
        self.assertTrue(is_error)
        self.assertIn("unbound", payload["error"])
        self.assertNotIn(("alpha", "check"), self.core.requests)

    def test_record_with_wrong_process_start_is_unbound(self):
        p = self.member(ALPHA, record="badstart")
        payload, is_error = self.tool(p, 1, "cbc_check")
        self.assertTrue(is_error)
        self.assertIn("unbound", payload["error"])

    def test_record_naming_another_pid_is_unbound(self):
        p = self.member(ALPHA, record="badpid")
        payload, is_error = self.tool(p, 1, "cbc_check")
        self.assertTrue(is_error)
        self.assertIn("unbound", payload["error"])

    def test_corroborated_session_without_a_grant_is_unregistered(self):
        p = self.member(STRANGER)
        payload, is_error = self.tool(p, 1, "cbc_check")
        self.assertTrue(is_error)
        self.assertIn("unregistered", payload["error"])

    def test_caller_supplied_session_id_is_ignored(self):
        s = socket.socket(socket.AF_UNIX)
        s.connect(self.router_sock)
        s.sendall((json.dumps({"v": 1, "op": "check", "session_id": ALPHA}) + "\n").encode())
        reply = json.loads(read_line(s))
        s.close()
        self.assertIn("error", reply)
        self.assertNotIn(("alpha", "check"), self.core.requests)

    def test_no_secret_reaches_logs(self):
        beta = self.member(BETA)
        self.tool(beta, 1, "cbc_send", {"room": "t1", "body": "x", "key": "k1"})
        logs = "".join(read_text(os.path.join(self.tmp, n)) for n in os.listdir(self.tmp) if n.endswith(".jsonl"))
        for g in self.grants:
            self.assertNotIn(g["token"], logs)
            self.assertNotIn(g["session"], logs)

    # --- adapter enforcement -----------------------------------------------------

    def test_initialize_lists_bound_tools_and_check_takes_no_arguments(self):
        p = self.member(ALPHA)
        init = self.rpc(p, 1, "initialize", {"protocolVersion": "2026-07-28"})
        self.assertEqual(init["result"]["protocolVersion"], "2025-06-18")
        tools = {t["name"]: t for t in self.rpc(p, 2, "tools/list")["result"]["tools"]}
        self.assertEqual(set(tools), {"cbc_check", "cbc_connect", "cbc_discover", "cbc_send"})
        self.assertEqual(tools["cbc_check"]["inputSchema"]["properties"], {})
        # Claude Code moves a longer result to a file the model may never read; a full core
        # batch, including one left pending under the old 128 KiB budget, must stay inline.
        limit = tools["cbc_check"].get("_meta", {}).get("anthropic/maxResultSizeChars", 50000)
        self.assertGreaterEqual(limit, LEGACY_BATCH_BYTES + 8192)
        self.assertLessEqual(limit, 500000)

    def test_discover_and_connect_use_corroborated_binding_without_receipts(self):
        p = self.member(ALPHA, extra_env={"CBC_CANDIDATE_CHANNEL": "1"})
        result, error = self.tool(p, 1, "cbc_discover")
        self.assertFalse(error, result)
        self.assertTrue(result["members"])
        self.assertEqual(self.core.operations[-1], ("alpha", "discover", {"limit": 32}))
        arguments = {"target": "beta", "kind": "peer", "scope_ref": "feature", "key": "connect-1"}
        result, error = self.tool(p, 2, "cbc_connect", arguments)
        self.assertFalse(error, result)
        self.assertEqual(result["connection"]["room"], "connected-route")
        self.assertEqual(self.core.operations[-1], ("alpha", "connect", arguments | {"explanation": ""}))
        self.assertNotIn(("alpha", "receipt"), self.core.requests)

    def test_discovery_and_connection_reject_identity_and_invalid_args_before_core(self):
        p = self.member(ALPHA, extra_env={"CBC_CANDIDATE_CHANNEL": "1"})
        arguments = {"target": "beta", "kind": "peer", "scope_ref": "feature", "key": "connect-1"}
        malformed = [("cbc_discover", {"member": "beta"}), ("cbc_discover", []),
                     ("cbc_connect", arguments | {"session": BETA}),
                     ("cbc_connect", arguments | {"kind": "admin"}),
                     ("cbc_connect", arguments | {"explanation": "x" * 601}),
                     ("cbc_connect", arguments | {"explanation": "😀" * 600}),
                     ("cbc_connect", arguments | {"kind": "report", "explanation": "override introduction"}),
                     ("cbc_connect", arguments | {"target": "\ud800"}),
                     ("cbc_connect", arguments | {"key": "\ncontrol"})]
        for rid, (name, value) in enumerate(malformed, 1):
            with self.subTest(name=name, value=repr(value)[:100]):
                reply = self.rpc(p, rid, "tools/call", {"name": name, "arguments": value})
                self.assertTrue(reply["result"]["isError"])
        self.assertEqual(self.core.requests, [])

    def test_check_with_arguments_is_rejected_before_reaching_core(self):
        p = self.member(ALPHA)
        self.assertTrue(self.wait_for(lambda: ("alpha", "check") in self.core.requests))  # startup hello
        _, is_error = self.tool(p, 1, "cbc_check", {"room": "t1"})
        self.assertTrue(is_error)
        self.assertEqual(self.core.requests.count(("alpha", "check")), 1)

    def test_send_arguments_are_enforced_not_just_declared(self):
        p = self.member(BETA)
        for bad in ({"room": "t1", "body": "b" * 16385, "key": "k"},
                    {"room": "t1", "body": "b", "key": "k", "sender": "alpha"},
                    {"room": "t1", "body": 7, "key": "k"}):
            _, is_error = self.tool(p, 1, "cbc_send", bad)
            self.assertTrue(is_error, bad)
        self.assertNotIn(("beta", "send"), self.core.requests)

    def test_body_at_the_escaped_limit_round_trips(self):
        alpha, beta = self.member(ALPHA), self.member(BETA)
        body = '"\\' * 8192  # 16 KiB raw; each character escapes to two bytes, 32 KiB exactly
        self.assertEqual(len(rust_json(body).encode()) - 2, MAX_ESCAPED_BODY)
        out, is_error = self.tool(beta, 1, "cbc_send", {"room": "t1", "body": body, "key": "esc"})
        self.assertFalse(is_error, out)
        payload, _ = self.tool(alpha, 1, "cbc_check")
        self.assertEqual([i["body"] for i in payload["items"]], [body])

    def test_ansi_and_control_text_is_ordinary_text(self):
        alpha, beta = self.member(ALPHA), self.member(BETA)
        body = "\x1b[31mFAIL\x1b[0m\tline\r\n\x07\x00\x08\x0c"
        out, is_error = self.tool(beta, 1, "cbc_send", {"room": "t1", "body": body, "key": "ansi"})
        self.assertFalse(is_error, out)
        payload, _ = self.tool(alpha, 1, "cbc_check")
        self.assertEqual([i["body"] for i in payload["items"]], [body])

    def test_body_over_the_escaped_limit_is_refused_before_core(self):
        beta = self.member(BETA)
        for body in ("\x01" + '"' * 16382,  # 16,383 raw bytes, 32,770 escaped
                     "\x01" * 16384):
            self.assertGreater(len(rust_json(body).encode()) - 2, MAX_ESCAPED_BODY)
            out, is_error = self.tool(beta, 1, "cbc_send", {"room": "t1", "body": body, "key": "big"})
            self.assertTrue(is_error)
            self.assertIn("escaped", out["error"])
        self.assertNotIn(("beta", "send"), self.core.requests)

    def test_body_with_a_lone_surrogate_is_refused_and_the_adapter_survives(self):
        beta = self.member(BETA)
        out, is_error = self.tool(beta, 1, "cbc_send", {"room": "t1", "body": "a\ud800b", "key": "sur"})
        self.assertTrue(is_error)
        self.assertNotIn(("beta", "send"), self.core.requests)
        out, is_error = self.tool(beta, 2, "cbc_send", {"room": "t1", "body": "fine", "key": "ok"})
        self.assertFalse(is_error, out)

    def test_batch_left_pending_under_the_old_budget_is_still_delivered(self):
        beta = self.member(BETA)
        body = "\u0080" * 8192
        for n in range(7):
            self.tool(beta, n + 1, "cbc_send", {"room": "t1", "body": body, "key": f"old{n}"})
        self.core.budget = LEGACY_BATCH_BYTES
        live = self.inbox("alpha")
        alpha = self.member(ALPHA, inbox=os.path.join(self.tmp, "in-alpha.sock"))
        self.assertTrue(self.wait_for(lambda: live.lines))
        self.core.budget = MAX_BATCH_BYTES
        payload, is_error = self.tool(alpha, 1, "cbc_check")
        self.assertFalse(is_error)
        self.assertEqual(len(payload["items"]), 7)

    def test_near_budget_multibyte_batch_round_trips_without_wedging(self):
        beta = self.member(BETA)
        body = "\u0080" * 8192  # 16 KiB of UTF-8; Python's default escaping would triple it
        item = {"sequence": 1, "room": "t1", "sender": "beta", "kind": "message", "body": body}
        fits = 1
        while len(rust_json([item] * (fits + 1)).encode()) <= MAX_BATCH_BYTES:
            fits += 1
        for n in range(fits + 1):
            out, is_error = self.tool(beta, n + 1, "cbc_send", {"room": "t1", "body": body, "key": f"u{n}"})
            self.assertFalse(is_error, out)
        live = self.inbox("alpha")
        alpha = self.member(ALPHA, inbox=os.path.join(self.tmp, "in-alpha.sock"))
        self.assertTrue(self.wait_for(lambda: live.lines))
        raw = self.rpc_raw(alpha, 1, "tools/call", {"name": "cbc_check", "arguments": {}})
        self.assertLessEqual(len(raw.encode()), 2 * MAX_BATCH_BYTES + 8192, "outer frame must keep UTF-8")
        result = json.loads(raw)["result"]
        self.assertFalse(result["isError"], result["content"][0]["text"][:200])
        first = json.loads(result["content"][0]["text"])["items"]
        self.assertEqual(len(first), fits, "batch should fill the core's byte budget, not freeze early")
        self.assertTrue(all(i["body"] == body for i in first))
        self.assertTrue(self.wait_for(lambda: ("alpha", "receipt") in self.core.requests))
        rest, _ = self.tool(alpha, 2, "cbc_check")
        self.assertEqual([i["body"] for i in rest["items"]], [body])

    def test_stdio_is_utf8_whatever_the_locale(self):
        beta = self.member(BETA)
        latin = {"LC_ALL": "en_US.ISO8859-1", "LANG": "en_US.ISO8859-1"}
        alpha = self.member(ALPHA, extra_env=latin)
        self.tool(beta, 1, "cbc_send", {"room": "t1", "body": "price \u20ac5", "key": "eur"})
        payload, is_error = self.tool(alpha, 1, "cbc_check")
        self.assertFalse(is_error, payload)
        self.assertEqual([i["body"] for i in payload["items"]], ["price \u20ac5"])
        out, is_error = self.tool(alpha, 2, "cbc_send", {"room": "t1", "body": "re \u20ac", "key": "eur2"})
        self.assertFalse(is_error, out)
        payload, _ = self.tool(beta, 2, "cbc_check")
        self.assertEqual([i["body"] for i in payload["items"]], ["re \u20ac"])

    def test_oversized_input_frame_is_refused_and_adapter_keeps_serving(self):
        p = self.member(ALPHA)
        p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "ping",
                                  "params": {"pad": "x" * 300000}}) + "\n")
        p.stdin.flush()
        self.assertIn("error", json.loads(self.readline(p)))
        self.assertEqual(self.rpc(p, 2, "ping")["result"], {})

    def test_connector_path_must_be_explicit(self):
        env = {**os.environ, "CLAUDE_CODE_SESSION_ID": ALPHA}
        env.pop("CBC_CANDIDATE_CONNECTOR", None)
        p = self.spawn([sys.executable, "-I", os.path.join(HERE, "adapter.py")], env)
        payload, is_error = self.tool(p, 1, "cbc_check")
        self.assertTrue(is_error)
        self.assertIn("not configured", payload["error"])


class AdapterWriteUnitTests(unittest.TestCase):
    def capture(self):
        import io
        return io.TextIOWrapper(io.BytesIO(), encoding="ascii")  # bytes must bypass the text layer

    def test_frame_over_the_outbound_cap_is_replaced_by_an_error_and_reported(self):
        out, real = self.capture(), sys.stdout
        sys.stdout = out
        try:
            ok = adapter.write({"jsonrpc": "2.0", "id": 9, "result": {"pad": "x" * adapter.MAX_OUT}})
        finally:
            sys.stdout = real
        self.assertFalse(ok)
        frame = json.loads(out.buffer.getvalue())
        self.assertEqual(frame["id"], 9)
        self.assertIn("error", frame)

    def test_frames_keep_utf8(self):
        out, real = self.capture(), sys.stdout
        sys.stdout = out
        try:
            self.assertTrue(adapter.write(adapter.text_result(1, {"items": [{"body": "\u0080"}]})))
        finally:
            sys.stdout = real
        raw = out.buffer.getvalue()
        self.assertNotIn(b"\\u0080", raw)
        self.assertIn("\u0080".encode(), raw)


class EventLogUnitTests(unittest.TestCase):
    def test_unwritable_event_log_never_raises(self):
        r = router.Router.__new__(router.Router)
        r.lock = threading.Lock()
        r.log_path = "/nonexistent-cbc-dir/router-events.jsonl"
        r.event("feed_failed", reply="error")  # the feed thread must survive a lost log


class ProfileUnitTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="cbcp-", dir="/tmp")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.path = os.path.join(self.tmp, "profile.json")

    def write(self, bindings, pad=0):
        fd = os.open(self.path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump({"v": 1, "core_socket": "/tmp/x.sock", "bindings": bindings, "pad": "x" * pad}, f)

    def binding(self, member, session):
        return {"context": "c", "member": member, "device": "d", "session": session, "epoch": 1, "token": "t"}

    def test_valid_profile_loads(self):
        self.write([self.binding("alpha", ALPHA), self.binding("beta", BETA)])
        core, bindings = router.load_profile(self.path)
        self.assertEqual(set(bindings), {ALPHA, BETA})

    def test_valid_profile_above_64k_loads(self):
        self.write([self.binding("alpha", ALPHA)], pad=70 * 1024)
        self.assertEqual(set(router.load_profile(self.path)[1]), {ALPHA})

    def test_profile_limit_counts_utf8_bytes_not_characters(self):
        self.write([self.binding("alpha", ALPHA)])
        with open(self.path, "w") as stream:
            json.dump({"v": 1, "core_socket": "/tmp/x.sock", "bindings": [self.binding("alpha", ALPHA)],
                       "pad": "界" * (50 * 1024)}, stream, ensure_ascii=False)
        with self.assertRaises(SystemExit):
            router.load_profile(self.path)

    def test_fifo_profile_is_refused_without_waiting_for_a_writer(self):
        os.mkfifo(self.path, 0o600)
        code = "import runpy,sys; runpy.run_path(sys.argv[1])['load_profile'](sys.argv[2])"
        result = subprocess.run([sys.executable, "-B", "-I", "-c", code,
            os.path.join(HERE, "router.py"), self.path],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=2)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"regular file", result.stderr)

    def test_duplicate_native_session_is_rejected(self):
        self.write([self.binding("alpha", ALPHA), self.binding("beta", ALPHA)])
        with self.assertRaises(SystemExit):
            router.load_profile(self.path)

    def test_more_bindings_than_one_feed_carries_is_rejected(self):
        self.write([self.binding(f"m{n}", f"s{n}") for n in range(router.MAX_BINDINGS)])
        self.assertEqual(len(router.load_profile(self.path)[1]), 64)
        os.unlink(self.path)
        self.write([self.binding(f"m{n}", f"s{n}") for n in range(router.MAX_BINDINGS + 1)])
        with self.assertRaises(SystemExit):
            router.load_profile(self.path)

    def test_hard_linked_profile_is_rejected(self):
        self.write([self.binding("alpha", ALPHA)])
        os.link(self.path, os.path.join(self.tmp, "second-name"))
        with self.assertRaises(SystemExit):
            router.load_profile(self.path)

    def test_oversized_profile_is_rejected(self):
        self.write([self.binding("alpha", ALPHA)], pad=router.MAX_PROFILE)
        with self.assertRaises(SystemExit):
            router.load_profile(self.path)


class CorroborationUnitTests(unittest.TestCase):
    """The race and uid checks need injected process facts; real processes cannot fake them."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="cbcu-", dir="/tmp")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.start = 1793044685
        with open(os.path.join(self.tmp, "500.json"), "w") as f:
            json.dump({"pid": 500, "sessionId": ALPHA,
                       "procStart": time.strftime("%a %b %d %H:%M:%S %Y", time.gmtime(self.start))}, f)

    def facts(self, starts):
        seq = iter(starts)
        def procinfo(pid):
            return {"ppid": 500, "uid": os.getuid(), "start": self.start} if pid == 400 else \
                   {"ppid": 1, "uid": os.getuid(), "start": next(seq)}
        return procinfo

    def test_stable_facts_corroborate(self):
        got = router.corroborate(400, os.getuid(), self.tmp, self.facts([self.start, self.start]))
        self.assertEqual(got, ALPHA)

    def test_parent_start_changing_during_lookup_is_rejected(self):
        self.assertIsNone(router.corroborate(400, os.getuid(), self.tmp, self.facts([self.start, self.start + 9])))

    def test_peer_of_another_uid_is_rejected(self):
        self.assertIsNone(router.corroborate(400, os.getuid() + 1, self.tmp, self.facts([self.start, self.start])))


class ChannelOwnershipUnitTests(unittest.TestCase):
    class Connection:
        closed = False
        def settimeout(self, _timeout):
            pass
        def sendall(self, _data):
            pass
        def close(self):
            self.closed = True

    def test_subscription_returns_its_request_slot_without_closing_the_channel(self):
        r = router.Router.__new__(router.Router)
        r.slots = threading.BoundedSemaphore(1)
        r.slots.acquire()
        r.handle = lambda _conn: router.SUBSCRIBED
        connection = self.Connection()
        r.serve(connection)
        self.assertFalse(connection.closed)
        self.assertTrue(r.slots.acquire(blocking=False), "an idle channel consumed request capacity")

    def test_initial_hint_failure_does_not_close_a_selector_owned_connection(self):
        r = router.Router.__new__(router.Router)
        r.lock = threading.Lock()
        r.channels, r.rung, r.channel_sessions = {}, {}, set()
        r.bindings = {ALPHA: {"member": "alpha"}}
        r.wake_write = type("Wake", (), {"send": lambda *_: 1})()
        r.event = lambda *_args, **_kwargs: None
        def fail(*_args, **_kwargs):
            raise OSError("fixture failure")
        r.notify = fail
        connection = self.Connection()
        self.assertIs(r.subscribe(ALPHA, connection), router.SUBSCRIBED)
        self.assertIs(r.channels[ALPHA][0], connection)
        self.assertFalse(connection.closed)


if __name__ == "__main__":
    unittest.main()
