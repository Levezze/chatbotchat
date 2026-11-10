#!/usr/bin/env python3
"""Native router between Claude Code adapters and the candidate Rust core.

One process for every chat on this machine. Each adapter connects over a private Unix
socket and sends only an operation, never an identity. The router derives the chat
from the connecting process itself:

    socket peer uid == ours, then peer pid (LOCAL_PEERPID / SO_PEERCRED),
    then the peer's parent pid, then ~/.claude/sessions/<parent>.json,
    whose pid and procStart must match the kernel's view of that parent,
    with the process facts read again after the lookup.

It then forwards to the core with that chat's binding from a private fixture profile.
The binding is per native session: an adapter restart reuses it and never rebinds.

Doorbells come from one shared feed. The router holds a single blocking watch on the core for
every binding in the profile, and the core answers it only when an unreceipted item
newer than the last answer exists. Any writer signals that feed, including adapters
that talk to the core directly, such as the Codex one. The core identifies changed members;
the router checks only their live chats and rings once per pending batch. Other chats are
untouched. The host-neutral feed module can dispatch to different native adapter callbacks
inside one device process; this experimental executable still implements Claude delivery only.

Trust boundary: the same uid. This stops model-chosen and cross-chat identity, not a
same-user process, which could write the sessions directory itself.

Usage: python3 -I router.py --profile ROOT/native-bindings.json --listen /tmp/x/router.sock
"""
import argparse, calendar, ctypes, hashlib, importlib.util, json, os, select, signal, socket, stat, struct, sys, threading, time

_feed_spec = importlib.util.spec_from_file_location(
    "cbc_candidate_feed", os.path.join(os.path.dirname(os.path.abspath(__file__)), "../shared/feed.py"))
_feed_module = importlib.util.module_from_spec(_feed_spec)
_feed_spec.loader.exec_module(_feed_module)
ChangeFeed = _feed_module.ChangeFeed
_target_spec = importlib.util.spec_from_file_location(
    "cbc_native_target", os.path.join(os.path.dirname(os.path.abspath(__file__)), "../shared/native_target.py"))
native_target = importlib.util.module_from_spec(_target_spec)
_target_spec.loader.exec_module(native_target)

MAX_FRAME = 256 * 1024
MAX_CLIENTS = 32
MAX_PROFILE = 128 * 1024
MAX_BINDINGS = 64  # connector.rs: one watch carries at most 64 bindings
AUTH_FIELDS = {"context", "member", "device", "session", "epoch", "token"}
OPS = {"hello": set(), "subscribe": set(), "check": set(), "receipt": {"reference"}, "send": {"room", "body", "key"},
       "discover": {"limit"}, "connect": {"target", "kind", "scope_ref", "key", "explanation"}}
DOORBELL = ("CBC_T1 doorbell #{n}: new CBC items are pending for this chat. "
            "Call the cbc_check tool with no arguments.")
SUBSCRIBED = object()


def dumps(value):
    """Compact UTF-8 JSON, the same bytes serde_json produces. Python's default
    ASCII escaping would triple a multibyte batch and break every frame bound."""
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def h(value):
    return hashlib.sha256(("cbc-router:" + value).encode()).hexdigest()[:16]


# --- process facts ----------------------------------------------------------------

if sys.platform == "darwin":
    _libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    _BSDINFO = struct.Struct("=12I16s32s5IiQQ")  # struct proc_bsdinfo

    def procinfo(pid):
        buf = ctypes.create_string_buffer(_BSDINFO.size)
        if _libproc.proc_pidinfo(pid, 3, 0, buf, _BSDINFO.size) != _BSDINFO.size:  # PROC_PIDTBSDINFO
            return None
        f = _BSDINFO.unpack(buf.raw)
        return {"ppid": f[4], "uid": f[5], "start": f[-2]}

    def peer_credentials(conn):
        uid = struct.unpack_from("=I", conn.getsockopt(0, 0x001, 76), 4)[0]  # SOL_LOCAL, LOCAL_PEERCRED
        pid = struct.unpack("=i", conn.getsockopt(0, 0x002, 4))[0]           # LOCAL_PEERPID
        return pid, uid
else:
    def procinfo(pid):
        try:
            with open(f"/proc/{pid}/stat") as f:
                fields = f.read().rsplit(")", 1)[1].split()
            with open(f"/proc/{pid}/status") as f:
                uid = int(next(l for l in f if l.startswith("Uid:")).split()[1])
            with open("/proc/stat") as f:
                btime = int(next(l for l in f if l.startswith("btime")).split()[1])
        except (OSError, StopIteration, IndexError, ValueError):
            return None
        return {"ppid": int(fields[1]), "uid": uid,
                "start": btime + int(fields[19]) // os.sysconf("SC_CLK_TCK")}

    def peer_credentials(conn):
        pid, uid, _ = struct.unpack("3i", conn.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
        return pid, uid


def record_start(value):
    try:
        return calendar.timegm(time.strptime(" ".join(str(value).split()), "%a %b %d %H:%M:%S %Y"))
    except ValueError:
        return None


def corroborate(peer_pid, peer_uid, sessions_dir, facts=procinfo):
    """Native session id of the chat whose adapter is this peer, or None."""
    me = os.getuid()
    if peer_uid != me:
        return None
    peer = facts(peer_pid)
    if not peer or peer["uid"] != me:
        return None
    parent_pid = peer["ppid"]
    parent = facts(parent_pid)
    try:
        with open(os.path.join(sessions_dir, f"{int(parent_pid)}.json")) as f:
            record = json.load(f)
    except (OSError, ValueError):
        return None
    if facts(peer_pid) != peer or facts(parent_pid) != parent or not parent:
        return None
    session = record.get("sessionId")
    if (record.get("pid") != parent_pid or parent["uid"] != me
            or record_start(record.get("procStart")) != parent["start"]
            or not isinstance(session, str) or not session):
        return None
    return session


# --- router -----------------------------------------------------------------------

def load_profile(path, with_targets=False):
    before = os.lstat(path)
    if not stat.S_ISREG(before.st_mode):
        raise SystemExit("profile must be a regular file")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as f:
        st = os.fstat(f.fileno())
        if ((st.st_dev, st.st_ino) != (before.st_dev, before.st_ino)
                or not stat.S_ISREG(st.st_mode) or st.st_uid != os.getuid() or st.st_mode & 0o777 != 0o600
                or st.st_nlink != 1):
            raise SystemExit("profile must be a single-link regular 0600 file owned by this user")
        raw = f.read(MAX_PROFILE + 1)
    if len(raw) > MAX_PROFILE:
        raise SystemExit("profile too large")
    try:
        profile = json.loads(raw)
    except ValueError:
        raise SystemExit("profile is not JSON")
    if (not isinstance(profile, dict) or type(profile.get("v")) is not int
            or profile.get("v") not in (1, 2) or not isinstance(profile.get("core_socket"), str)
            or not isinstance(profile.get("bindings"), list)):
        raise SystemExit("unsupported profile")
    if len(profile.get("bindings", [])) > MAX_BINDINGS:
        raise SystemExit(f"profile has more than {MAX_BINDINGS} bindings, more than one feed carries")
    bindings = {}
    for b in profile.get("bindings", []):
        if (not isinstance(b, dict) or set(b) != AUTH_FIELDS
                or not isinstance(b.get("session"), str) or not b["session"]):
            raise SystemExit("malformed binding in profile")
        if b["session"] in bindings:
            raise SystemExit("duplicate native session in profile")
        bindings[b["session"]] = b
    try:
        targets = native_target.parse_targets(profile, bindings)
    except ValueError as error:
        raise SystemExit(str(error)) from error
    if with_targets:
        return profile["core_socket"], bindings, targets
    return profile["core_socket"], bindings


def read_frame(conn):
    buf = b""
    while not buf.endswith(b"\n"):
        chunk = conn.recv(65536)
        if not chunk:
            break
        buf += chunk
        if len(buf) > MAX_FRAME:
            return None
    return buf if buf.endswith(b"\n") else None


class Router:
    def __init__(self, args):
        self.core, bindings, self.native_targets = load_profile(args.profile, with_targets=True)
        # A revoked binding is quarantined, not fatal: one stale grant in the
        # profile must not take every other chat on this device down with it.
        self.revoked = self.verify_target_registry(bindings, self.native_targets)
        self.bindings = {s: b for s, b in bindings.items() if s not in self.revoked}
        self.sessions_dir = args.sessions_dir
        self.log_path = os.path.join(args.log_dir, "router-events.jsonl") if args.log_dir else None
        self.bells = 0
        self.rung = {}  # session -> reference of the pending batch last rung for
        self.lock = threading.Lock()
        if self.revoked:
            self.event("bindings_quarantined", count=len(self.revoked))
        self.slots = threading.BoundedSemaphore(MAX_CLIENTS)
        self.channels = {}
        self.channel_sessions = set()
        self.wake_read, self.wake_write = socket.socketpair()
        self.wake_read.setblocking(False)
        self.wake_write.setblocking(False)
        self.feed = ChangeFeed(self.core, self.bindings.values(), {
            (binding["context"], binding["member"]):
                (lambda _hint, session=session: self.deliver_hint(session))
            for session, binding in self.bindings.items()
        }, self.event) if self.bindings else None

    def verify_target_registry(self, bindings, targets):
        # Epoch targets are immutable in the core. This handshake is cached by
        # the installed in-memory registry until a fully verified profile refresh.
        # Returns the sessions the core definitively revoked; any other
        # disagreement, including an unreachable core, still fails closed.
        revoked = set()
        for session, binding in bindings.items():
            status = self.core_call(binding, {"op": "binding_status"})
            if native_target.is_revoked(status):
                revoked.add(session)
            elif not native_target.target_agrees(targets[session]["target"], status):
                raise ValueError("native target disagrees with core binding authority")
        return revoked

    def event(self, kind, **fields):
        if not self.log_path:
            return
        try:  # a lost log must never take a delivery path down with it
            with self.lock, open(self.log_path, "a") as f:
                f.write(dumps({"t": time.time(), "event": kind, **fields}) + "\n")
        except OSError:
            pass

    def core_call(self, binding, request):
        s = socket.socket(socket.AF_UNIX)
        s.settimeout(30)
        try:
            s.connect(self.core)
            s.sendall((dumps({"v": 1, "auth": binding, "request": request}) + "\n").encode())
            raw = read_frame(s)
            return json.loads(raw) if raw else {"type": "error", "message": "core reply missing or too large"}
        except (OSError, ValueError):
            return {"type": "error", "message": "core unavailable"}
        finally:
            s.close()

    def follow_feed(self):
        """One host-neutral feed dispatches only members whose unread state changed."""
        if self.feed:
            self.feed.run()

    def deliver_hint(self, session):
        """False leaves the hint queued for shared backoff, without a new model turn."""
        reply = self.notify(session)
        return reply.get("type") == "empty" or reply.get("_notified") is True

    def target_allowed(self, session, pid, uid, direct=False):
        selected = self.native_targets.get(session)
        if not selected or selected["target"] is None:
            return selected is not None  # Explicit v1 legacy compatibility only.
        if selected["host"] != "claude":
            return False
        peer = procinfo(pid)
        if not peer:
            return False
        host_pid = pid if direct else peer.get("ppid")
        observed = native_target.claude_observation(host_pid, uid, self.sessions_dir,
            procinfo, record_start, adapter_pid=None if direct else pid)
        return (observed is not None and observed["session"] == session
                and native_target.match_target(selected["target"], observed))

    def delivery_target_allowed(self, session):
        selected = self.native_targets.get(session)
        if selected and selected["target"] is None:
            return True
        with self.lock:
            channel = self.channels.get(session)
        if not channel:
            return False
        try:
            pid, uid = peer_credentials(channel[0])
            return self.target_allowed(session, pid, uid)
        except OSError:
            return False

    def subscribe(self, session, conn):
        """One existing MCP shim connection, bound by peer identity rather than an argument."""
        entry = (conn, threading.Lock())
        conn.settimeout(3)
        with self.lock:
            if session in self.channels:
                return {"error": "notification channel already connected"}
            conn.sendall(b'{"type":"subscribed"}\n')
            self.channel_sessions.add(session)
            self.channels[session] = entry
            self.rung.pop(session, None)
        try:
            self.wake_write.send(b"1")  # Rebuild the one device selector after registration.
        except BlockingIOError:
            pass
        try:
            # Registration cleared the old connection's latch. Normal deduplication
            # avoids a second hint if the shared feed has already rung this connection.
            self.notify(session)
        except Exception as error:
            # Registration already transferred socket ownership to the selector. Leave
            # recovery to the shared feed rather than closing a descriptor under select().
            self.event("channel_initial_hint_failed", member=self.bindings[session]["member"],
                       reason=type(error).__name__)
        return SUBSCRIBED

    def channel_sockets(self):
        with self.lock:
            return [entry[0] for entry in self.channels.values()]

    def drop_channel(self, conn):
        with self.lock:
            for session, entry in list(self.channels.items()):
                if entry[0] is conn:
                    del self.channels[session]
                    self.rung.pop(session, None)
        conn.close()

    def live_socket(self, session):
        """Fresh lookup every time: a relaunched chat has a new pid and socket."""
        for name in os.listdir(self.sessions_dir):
            if not name.endswith(".json"):
                continue
            try:
                with open(os.path.join(self.sessions_dir, name)) as f:
                    record = json.load(f)
                if record.get("sessionId") != session:
                    continue
                os.kill(int(record["pid"]), 0)
                path = record["messagingSocketPath"]
                if stat.S_ISSOCK(os.lstat(path).st_mode):
                    return path
            except (OSError, ValueError, KeyError, TypeError):
                continue
        return None

    def ring(self, session, path=None):
        if session not in self.bindings:  # only bound chats are ever rung
            return False
        with self.lock:
            channel = self.channels.get(session)
            channel_only = session in self.channel_sessions
        if channel:
            try:
                with channel[1]:
                    channel[0].sendall(b'{"type":"changed"}\n')
                self.event("channel_written", member=self.bindings[session]["member"])
                return True
            except OSError:
                self.event("channel_failed", member=self.bindings[session]["member"])
                return False
        if channel_only:
            return False  # Never fall back to a private messaging socket after channel opt-in.
        path = path or self.live_socket(session)
        member = self.bindings[session]["member"]
        if not path:
            self.event("doorbell_skipped", member=member, reason="no live chat process")
            return False
        with self.lock:
            self.bells += 1
            n = self.bells
        frame = dumps({"type": "user", "message": {"role": "user", "content": DOORBELL.format(n=n)}}) + "\n"
        s = socket.socket(socket.AF_UNIX)
        s.settimeout(3)
        try:
            s.connect(path)
            s.sendall(frame.encode())
            self.event("doorbell_written", member=member, n=n)
            return True
        except OSError as e:
            self.event("doorbell_failed", member=member, error=type(e).__name__)
            return False
        finally:
            s.close()

    def notify(self, session, force=False):
        """Ring a chat only if the core has a batch pending for it, once per batch.

        force (hello) rings even for a batch already rung, because the chat's process
        may have been gone when that bell was sent. A bell that is not written is
        forgotten, so the next send or hello tries again. A chat with no live process
        is not checked at all: checking would freeze its next batch at whatever is
        pending now, and its hello rings it on relaunch anyway."""
        if not self.delivery_target_allowed(session):
            return {"type": "error", "message": "native execution target mismatch"}
        with self.lock:
            channel_only = session in self.channel_sessions
            channel_live = session in self.channels
        path = None if channel_only else self.live_socket(session)
        if not (channel_live if channel_only else path) and not force:
            self.event("doorbell_skipped", member=self.bindings[session]["member"], reason="no live chat process")
            return {"type": "skipped"}
        reply = self.core_call(self.bindings[session], {"op": "check"})
        if reply.get("type") != "batch":
            return reply
        reference = reply["reference"]
        with self.lock:
            if not force and self.rung.get(session) == reference:
                reply["_notified"] = True
                return reply
            self.rung[session] = reference
        reply["_notified"] = self.ring(session, path)
        if not reply["_notified"]:
            with self.lock:
                if self.rung.get(session) == reference:
                    del self.rung[session]
        return reply

    def handle(self, conn):
        pid, uid = peer_credentials(conn)
        return self.handle_request(conn, pid, uid)

    def handle_request(self, conn, pid, uid, raw=None):
        """Handle one Claude request; a common device listener may supply its frame."""
        session = corroborate(pid, uid, self.sessions_dir)
        if not session:
            self.event("peer_rejected", reason="unbound")
            return {"error": "unbound: chat process not corroborated"}
        binding = self.bindings.get(session)
        if not binding:
            self.event("peer_rejected", reason="unregistered", session=h(session))
            return {"error": "unregistered: no binding for this chat"}
        if not self.target_allowed(session, pid, uid):
            return {"error": "native execution target mismatch"}
        raw = read_frame(conn) if raw is None else raw
        try:
            req = json.loads(raw) if raw else None
            op = req.pop("op")
            assert req.pop("v") == 1 and set(req) == OPS[op]
        except (TypeError, ValueError, KeyError, AssertionError, AttributeError):
            return {"error": "invalid request"}
        member = binding["member"]
        if op == "subscribe":
            return self.subscribe(session, conn)
        if op == "hello":
            reply = self.notify(session, force=True)
            kind = reply.get("type")
            self.event("hello", member=member, pending=kind == "batch", reply=kind)
            if kind not in ("batch", "empty"):
                return {"error": str(reply.get("message", "core error"))}
            return {"ok": True}
        reply = self.core_call(binding, {"op": op, **req})
        kind = reply.get("type")
        self.event(op, member=member, reply=kind)
        if op == "receipt":
            # Ring again if anything is still pending: a batch holds at most 32 items, and
            # a failed receipt (the core cancelled or superseded the batch) leaves the same
            # items waiting for redelivery with nothing else to wake the chat.
            with self.lock:
                self.rung.pop(session, None)
            self.notify(session)
        if kind == "batch":
            return {"items": reply["items"], "reference": reply["reference"]}
        if kind == "empty":
            return {"items": []}
        if kind == "receipted":
            return {"ok": True}
        if kind == "sent":  # the core feed rings the readers
            return {"sequence": reply["sequence"]}
        if kind == "directory":
            return {"members": reply["members"]}
        if kind == "connected":
            return {"connection": reply["connection"]}
        return {"error": str(reply.get("message", "core error"))}

    def serve(self, conn):
        keep = False
        try:
            conn.settimeout(10)
            reply = self.handle(conn)
            if reply is SUBSCRIBED:
                keep = True  # The main selector owns it; no parked thread or request slot.
            elif reply is not None:
                conn.sendall((dumps(reply) + "\n").encode())
        except OSError:
            pass
        finally:
            if not keep:
                self.drop_channel(conn)
            self.slots.release()


def listen(path):
    if len(path.encode()) >= 104:
        raise SystemExit("listen path too long for a Unix socket")
    if os.path.lexists(path):
        probe = socket.socket(socket.AF_UNIX)
        try:
            if not stat.S_ISSOCK(os.lstat(path).st_mode) or os.lstat(path).st_uid != os.getuid():
                raise SystemExit("listen path exists and is not our socket")
            probe.connect(path)
            raise SystemExit("another router is already listening")
        except ConnectionRefusedError:
            os.unlink(path)  # stale socket from a dead router
        finally:
            probe.close()
    srv = socket.socket(socket.AF_UNIX)
    old = os.umask(0o177)
    try:
        srv.bind(path)
    finally:
        os.umask(old)
    srv.listen(MAX_CLIENTS)
    return srv, os.lstat(path).st_ino


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--profile", required=True)
    ap.add_argument("--listen", required=True)
    ap.add_argument("--sessions-dir", default=os.path.expanduser("~/.claude/sessions"))
    ap.add_argument("--log-dir")
    ap.add_argument("--ttl", type=int, help="optional disposable-test lifetime; otherwise runs until stopped")
    args = ap.parse_args()
    if args.ttl is not None and args.ttl <= 0:
        ap.error("ttl must be positive")
    router = Router(args)
    if os.path.abspath(args.listen) == os.path.abspath(router.core):
        raise SystemExit("router socket must differ from the core socket")
    srv, inode = listen(args.listen)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    router.event("listening", pid=os.getpid(), members=sorted(b["member"] for b in router.bindings.values()))
    if router.bindings:
        threading.Thread(target=router.follow_feed, daemon=True).start()
    deadline = time.monotonic() + args.ttl if args.ttl is not None else None
    try:
        while True:
            remaining = deadline - time.monotonic() if deadline is not None else None
            if remaining is not None and remaining <= 0:
                break
            ready = select.select([srv, router.wake_read, *router.channel_sockets()], [], [], remaining)[0]
            if not ready:
                continue
            for conn in ready:
                if conn is router.wake_read:
                    try:
                        conn.recv(4096)
                    except BlockingIOError:
                        pass
                elif conn is not srv:
                    # Subscriptions accept no further input. EOF or extra bytes retire it.
                    router.drop_channel(conn)
            if srv not in ready:
                continue
            try:
                conn, _ = srv.accept()
            except OSError:
                continue
            if not router.slots.acquire(blocking=False):
                conn.close()
                continue
            threading.Thread(target=router.serve, args=(conn,), daemon=True).start()
        router.event("ttl_exit")
    finally:
        if router.feed:
            router.feed.stop()
        for conn in router.channel_sockets():
            router.drop_channel(conn)
        router.wake_read.close()
        router.wake_write.close()
        srv.close()
        try:
            if os.lstat(args.listen).st_ino == inode:
                os.unlink(args.listen)
        except OSError:
            pass


if __name__ == "__main__":
    main()
