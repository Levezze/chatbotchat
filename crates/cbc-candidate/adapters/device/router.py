#!/usr/bin/env python3
"""One candidate device process: shared feed, Claude channel server, Codex queue hints.

Explicit private profiles only. Native app support is qualified separately; accepting a
hint never receipts an inbox batch or asserts that a model handled it.
"""
import argparse
import hmac
import importlib.util
import json
import os
from pathlib import Path
import re
import select
import signal
import stat
import threading


ADAPTERS = Path(__file__).resolve().parents[1]


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


claude = load_module("cbc_device_claude", ADAPTERS / "claude/router.py")
shared = load_module("cbc_device_feed", ADAPTERS / "shared/feed.py")
codex = load_module("cbc_device_codex", ADAPTERS / "codex/queue_delivery.py")


def load_targets(path):
    if not Path(path).is_absolute():
        raise ValueError("device profile path must be explicit and absolute")
    before = os.lstat(path)
    if not stat.S_ISREG(before.st_mode):
        raise ValueError("device profile must be a regular file")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        if ((info.st_dev, info.st_ino) != (before.st_dev, before.st_ino)
                or not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_mode & 0o777 != 0o600 or info.st_nlink != 1):
            raise ValueError("device profile must be private, owned, regular and single-link")
        raw = stream.read(claude.MAX_PROFILE + 1)
    if len(raw) > claude.MAX_PROFILE:
        raise ValueError("device profile too large")
    profile = json.loads(raw)
    if (not isinstance(profile, dict) or set(profile) != {"v", "core_socket", "bindings", "targets"}
            or type(profile["v"]) is not int or profile["v"] != 1 or not isinstance(profile["bindings"], list)
            or not 1 <= len(profile["bindings"]) <= shared.MAX_BINDINGS
            or not isinstance(profile["core_socket"], str)
            or not os.path.isabs(profile["core_socket"]) or not isinstance(profile["targets"], list)):
        raise ValueError("unsupported device profile")
    if Path(profile["core_socket"]) != Path(path).absolute().parent / "connector.sock":
        raise ValueError("core socket must be the candidate socket beside this private profile")
    bindings = {}
    members = set()
    for binding in profile["bindings"]:
        if (not isinstance(binding, dict) or set(binding) != shared.AUTH_FIELDS
                or not all(isinstance(binding[k], str) and 0 < len(binding[k]) <= 128
                           and not any(ord(c) < 32 for c in binding[k])
                           for k in ("context", "member", "device", "session"))
                or type(binding["epoch"]) is not int or binding["epoch"] < 1
                or not isinstance(binding["token"], str)
                or not re.fullmatch(r"cbcc1_[0-9a-fA-F]{64}", binding["token"])):
            raise ValueError("invalid device binding")
        session = binding["session"]
        key = (binding["context"], binding["member"])
        if session in bindings or key in members:
            raise ValueError("duplicate device binding")
        bindings[session] = binding
        members.add(key)
    targets = {}
    for target in profile["targets"]:
        if (not isinstance(target, dict) or set(target) != {"session", "host"}
                or not isinstance(target["session"], str)
                or target["host"] not in ("codex", "claude")
                or target["session"] in targets):
            raise ValueError("invalid device target")
        targets[target["session"]] = target["host"]
    if set(targets) != set(bindings):
        raise ValueError("every binding needs exactly one explicit host target")
    return profile["core_socket"], bindings, targets


class DeviceRouter(claude.Router):
    def __init__(self, args):
        core, bindings, targets = load_targets(args.profile)
        queued = {}
        # Validate all host targets before the inherited router allocates sockets.
        for session, host in targets.items():
            if host == "codex":
                if not args.codex_executable:
                    raise ValueError("an explicit Codex executable is required for Codex targets")
                queued[session] = codex.QueueDelivery(args.codex_executable, session)
        super().__init__(args)
        if self.core != core or self.bindings != bindings:
            self.wake_read.close()
            self.wake_write.close()
            raise ValueError("device profile changed during startup")
        self.targets = targets
        # The inherited local request server handles only corroborated Claude shims.
        # Codex's existing MCP adapter continues to call the core directly.
        self.bindings = {session: binding for session, binding in bindings.items()
                         if targets[session] == "claude"}
        # This driver supports the documented channel path only, including before a
        # shim subscribes. Never let a missing channel select the historical socket probe.
        self.channel_sessions = set(self.bindings)
        self.registry_lock = threading.RLock()
        self.profile_path = args.profile
        self.codex_executable = args.codex_executable
        self.queued = queued
        self.all_bindings = bindings
        self.quarantined = set()
        handlers = {}
        for session, binding in bindings.items():
            key = (binding["context"], binding["member"])
            if targets[session] == "claude":
                handlers[key] = lambda _hint, session=session: self.deliver_hint(session)
            else:
                queued[session].event = self.event
                queued[session].status_provider = lambda binding=binding: self.core_call(
                    binding, {"op": "binding_status"})
                handlers[key] = queued[session]
        self.handlers = handlers

        def policy(key):
            with self.registry_lock:
                active = [b for session, b in self.all_bindings.items() if session not in self.quarantined]
                binding = next(b for b in active if key is None or (b["context"], b["member"]) == key)
            reply = self.core_call(binding, {"op": "settings"})
            if not isinstance(reply, dict) or reply.get("type") != "settings":
                raise ValueError("runtime settings unavailable")
            values = reply.get("policy")
            if not isinstance(values, dict):
                raise ValueError("invalid runtime settings")
            return {"retry_initial_ms": values.get("retry_initial_ms"),
                    "retry_max_ms": values.get("retry_max_ms"), "revision": reply.get("revision")}

        self.feed = shared.ChangeFeed(core, bindings.values(), handlers, self.event,
                                      policy_provider=policy, reconcile=self.reconcile_bindings)

    def install_registry(self):
        """Caller holds registry_lock; one shared feed survives membership changes."""
        active = {s: b for s, b in self.all_bindings.items() if s not in self.quarantined}
        with self.lock:
            self.bindings = {s: b for s, b in active.items() if self.targets[s] == "claude"}
            self.channel_sessions = set(self.bindings)
        handlers = {(b["context"], b["member"]): self.handlers[(b["context"], b["member"])]
                    for b in active.values()}
        self.feed.replace_registry(active.values(), handlers)

    def reconcile_bindings(self):
        """A rejected watch cannot let one revoked binding starve healthy chats."""
        with self.registry_lock:
            candidates = [(s, b) for s, b in self.all_bindings.items() if s not in self.quarantined]
        invalid = []
        for session, binding in candidates:
            reply = self.core_call(binding, {"op": "binding_status"})
            if reply.get("type") == "error" and reply.get("code") == "invalid_binding":
                invalid.append((session, binding))
            elif reply.get("type") != "binding_status":
                # Core downtime is not revocation. Avoid N sequential timeout probes.
                break
        with self.registry_lock:
            removed = [s for s, b in invalid if self.all_bindings.get(s) == b and s not in self.quarantined]
            if not removed:
                return False
            self.quarantined.update(removed)
            self.install_registry()
        self.event("bindings_quarantined", count=len(removed))
        return True

    def refresh_profile(self):
        """Explicit SIGHUP lifecycle refresh; reject malformed profiles atomically."""
        core, bindings, targets = load_targets(self.profile_path)
        if core != self.core:
            raise ValueError("profile refresh cannot change the core endpoint")
        queued, handlers = {}, {}
        with self.registry_lock:
            for session, binding in bindings.items():
                key = (binding["context"], binding["member"])
                if targets[session] == "claude":
                    handlers[key] = lambda _hint, session=session: self.deliver_hint(session)
                else:
                    if not self.codex_executable:
                        raise ValueError("Codex executable missing")
                    prior = self.queued.get(session)
                    callback = (prior if prior and self.all_bindings.get(session) == binding
                                else codex.QueueDelivery(self.codex_executable, session, self.event))
                    callback.status_provider = lambda binding=binding: self.core_call(binding, {"op": "binding_status"})
                    queued[session] = handlers[key] = callback
            self.all_bindings, self.targets, self.queued, self.handlers = bindings, targets, queued, handlers
            self.quarantined = set()
            self.install_registry()
        self.event("profile_refreshed", targets=len(bindings))

    def handle(self, conn):
        pid, uid = claude.peer_credentials(conn)
        raw = claude.read_frame(conn)
        try:
            request = json.loads(raw) if raw else None
        except ValueError:
            return {"error": "invalid request"}
        if isinstance(request, dict) and request.get("op") == "codex_receipt":
            return self.receipt_nudge(request, uid)
        return self.handle_request(conn, pid, uid, raw=raw)

    def receipt_nudge(self, request, uid):
        """Core facts rearm hints. A caller cannot claim a receipt or supply content."""
        if (uid != os.getuid() or set(request) != {"v", "op", "auth"}
                or type(request["v"]) is not int or request["v"] != 1
                or request["op"] != "codex_receipt" or not isinstance(request["auth"], dict)
                or set(request["auth"]) != shared.AUTH_FIELDS):
            return {"error": "invalid device lifecycle request"}
        auth = request["auth"]
        if (not all(isinstance(auth[k], str) for k in ("context", "member", "device", "session", "token"))
                or type(auth["epoch"]) is not int
                or not re.fullmatch(r"cbcc1_[0-9a-fA-F]{64}", auth["token"])):
            return {"error": "invalid device lifecycle binding"}
        with self.registry_lock:
            binding = self.all_bindings.get(auth.get("session"))
            if (not binding or auth["session"] in self.quarantined
                    or self.targets.get(auth["session"]) != "codex"
                    or not isinstance(auth.get("token"), str)
                    or not hmac.compare_digest(auth["token"], binding["token"])
                    or any(auth.get(k) != binding[k] for k in shared.AUTH_FIELDS - {"token"})):
                return {"error": "unregistered device lifecycle request"}
            callback = self.queued[auth["session"]]
        # QueueDelivery obtains binding_status itself; no caller-supplied sequence is trusted.
        state = self.core_call(binding, {"op": "binding_status"})
        if state.get("type") != "binding_status":
            return {"error": "binding status unavailable"}
        first = state.get("first_unread_sequence")
        if first is None:
            with callback.lock:
                callback.accepted = None
            return {"ok": True}
        if type(first) is not int or first < 1:
            return {"error": "invalid binding status"}
        try:
            self.feed.dispatch({"type": "members_changed", "sequence": first, "members": [{
                "context": binding["context"], "member": binding["member"], "sequence": first}]}, 0)
        except (ValueError, OSError):
            return {"error": "device registry changed"}
        return {"ok": True}


def run(router, listen_path, stopped=None):
    if (Path(listen_path).parent != Path(router.core).parent
            or not Path(listen_path).is_absolute()):
        raise ValueError("device socket must be inside the private candidate root")
    if os.path.abspath(listen_path) == os.path.abspath(router.core):
        raise ValueError("device socket must differ from the candidate core socket")
    stopped = stopped or threading.Event()
    refresh = threading.Event()
    server, inode = claude.listen(listen_path)
    follower = threading.Thread(target=router.follow_feed, name="cbc-shared-device-feed")

    def stop(*_args):
        stopped.set()
        router.feed.stop()
        try:
            router.wake_write.send(b"1")
        except OSError:
            pass

    if threading.current_thread() is threading.main_thread():
        signal.signal(signal.SIGTERM, stop)
        signal.signal(signal.SIGINT, stop)
        def request_refresh(*_args):
            refresh.set()
            try:
                router.wake_write.send(b"1")
            except OSError:
                pass
        signal.signal(signal.SIGHUP, request_refresh)
    follower.start()
    try:
        while not stopped.is_set():
            ready = select.select([server, router.wake_read, *router.channel_sockets()], [], [])[0]
            for connection in ready:
                if connection is router.wake_read:
                    try:
                        connection.recv(4096)
                    except BlockingIOError:
                        pass
                    if refresh.is_set():
                        refresh.clear()
                        try:
                            router.refresh_profile()
                        except (OSError, ValueError):
                            router.event("profile_refresh_failed")
                elif connection is not server:
                    router.drop_channel(connection)
            if server in ready and not stopped.is_set():
                connection, _ = server.accept()
                if router.slots.acquire(blocking=False):
                    threading.Thread(target=router.serve, args=(connection,), daemon=True).start()
                else:
                    connection.close()
    finally:
        stop()
        follower.join()
        for connection in router.channel_sockets():
            router.drop_channel(connection)
        router.wake_read.close()
        router.wake_write.close()
        server.close()
        try:
            if os.lstat(listen_path).st_ino == inode:
                os.unlink(listen_path)
        except OSError:
            pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--listen", required=True)
    parser.add_argument("--sessions-dir", required=True,
                        help="actual Claude session corroboration directory")
    parser.add_argument("--codex-executable", help="explicit installed Codex CLI path")
    parser.add_argument("--log-dir", help="candidate-only private diagnostics directory")
    args = parser.parse_args()
    try:
        router = DeviceRouter(args)
        run(router, args.listen)
    except (OSError, ValueError) as error:
        parser.exit(1, f"device router unavailable: {type(error).__name__}\n")


if __name__ == "__main__":
    main()
