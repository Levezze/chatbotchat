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
import queue
import re
import select
import signal
import socket
import stat
import threading
import time


ADAPTERS = Path(__file__).resolve().parents[1]


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


claude = load_module("cbc_device_claude", ADAPTERS / "claude/router.py")
shared = load_module("cbc_device_feed", ADAPTERS / "shared/feed.py")
codex = load_module("cbc_device_codex", ADAPTERS / "codex/queue_delivery.py")
native = load_module("cbc_device_native_http", ADAPTERS / "device/native_http.py")


def load_targets(path, with_native=False):
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
            or type(profile["v"]) is not int or profile["v"] not in (1, 2) or not isinstance(profile["bindings"], list)
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
    native_targets = claude.native_target.parse_targets(profile, bindings)
    targets = {session: entry["host"] for session, entry in native_targets.items()}
    if with_native:
        return profile["core_socket"], bindings, targets, native_targets
    return profile["core_socket"], bindings, targets


class DeviceRouter(claude.Router):
    def __init__(self, args):
        core, bindings, targets, native_targets = load_targets(args.profile, with_native=True)
        queued = {}
        # Validate all host targets before the inherited router allocates sockets.
        for session, host in targets.items():
            if host == "codex":
                if not args.codex_executable:
                    raise ValueError("an explicit Codex executable is required for Codex targets")
                queued[session] = codex.QueueDelivery(args.codex_executable, session)
        super().__init__(args)
        active = {s: b for s, b in bindings.items() if s not in self.revoked}
        if self.core != core or self.bindings != active or self.native_targets != native_targets:
            self.wake_read.close()
            self.wake_write.close()
            raise ValueError("device profile changed during startup")
        self.targets = targets
        # The inherited local request server handles only corroborated Claude shims.
        # Codex's existing MCP adapter continues to call the core directly.
        self.bindings = {session: binding for session, binding in active.items()
                         if targets[session] == "claude"}
        # This driver supports the documented channel path only, including before a
        # shim subscribes. Never let a missing channel select the historical socket probe.
        self.channel_sessions = set(self.bindings)
        self.registry_lock = threading.RLock()
        self.profile_path = args.profile
        self.codex_executable = args.codex_executable
        self.queued = queued
        self.all_bindings = bindings
        self.quarantined = set(self.revoked)
        self.native_ready = threading.Condition()
        self.native_generation = 0
        self.native_sessions = set()
        self.native_pending = set()
        self.native_slots = threading.BoundedSemaphore(native.MAX_WAITERS)
        # #113 B1. Reports leave the delivery path through one bounded queue and one worker,
        # so a slow or absent core can never delay or fail a delivery.
        self.wake_reports = queue.Queue(maxsize=256)
        # The core counts recovery attempts from wake reports, which can be lost. A hold is
        # registered before each recovery and keeps that binding's oldest unread item from being
        # recovered again until the core acknowledges the attempt's report. It is dropped at
        # once if no attempt was reported. An ack lifts only the hold its report marked.
        # (Codex reviews 3, 4 and 5.)
        self.recovery_lock = threading.Lock()
        self.recovery_holds = {}
        self.wake_sender = lambda grant, request: self.bounded_call(grant, request, 2.0)
        self.latest_hint = {}
        self.native_recover = set()
        self.recovery_interval = 60.0
        handlers = {}
        for session, binding in bindings.items():
            key = (binding["context"], binding["member"])
            if targets[session] == "claude":
                handlers[key] = self.claude_handler(session)
            else:
                queued[session].event = self.event
                queued[session].status_provider = lambda binding=binding: self.core_call(
                    binding, {"op": "binding_status"})
                queued[session].reporter = self.codex_reporter(binding)
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
        if self.quarantined:
            with self.registry_lock:
                self.install_registry()
        threading.Thread(target=self.report_worker, name="cbc-wake-reports", daemon=True).start()

    def claude_handler(self, session):
        def handler(hint):
            sequence = hint.get("sequence") if isinstance(hint, dict) else None
            if type(sequence) is int and sequence > 0:
                with self.native_ready:
                    self.latest_hint[session] = max(sequence, self.latest_hint.get(session, 0))
            return self.deliver_hint(session)
        return handler

    def codex_reporter(self, binding):
        return lambda through, outcome, failure, recovery, attempted_us: self.report_wake(
            binding, through, "codex_queue", outcome, failure, recovery, attempted_us)

    def report_wake(self, binding, through, kind, outcome, failure, recovery, attempted_us):
        """Enqueue one wake attempt for the core. Never blocks and never raises.

        attempted_us is the wall clock in microseconds taken before the host was notified, so
        the core can tell which items were still unread at the attempt."""
        if type(through) is not int or through < 1:
            return
        request = {"op": "wake", "through": through, "kind": kind, "outcome": outcome,
                   "recovery": bool(recovery), "attempt_wall_us": attempted_us}
        if failure is not None:
            request["failure"] = failure
        hold = None
        if recovery:
            with self.recovery_lock:
                hold = self.recovery_holds.get((binding["context"], binding["member"]))
                if hold is not None:
                    hold["reported"] = True
        try:
            self.wake_reports.put_nowait((dict(binding), request, hold))
        except queue.Full:
            self.event("wake_report_dropped", member=binding["member"])

    def report_worker(self):
        while True:
            try:
                grant, request, hold = self.wake_reports.get(timeout=0.2)
            except queue.Empty:
                if self.feed.stopped.is_set():
                    return
                continue
            try:
                reply = self.wake_sender(grant, request)
                if not isinstance(reply, dict) or reply.get("type") != "woken":
                    self.event("wake_report_failed", member=grant["member"], reason="rejected")
                elif hold is not None:
                    # Only the hold this report marked: a late ack for an older item must not
                    # lift a newer item's hold. (Codex review 5.)
                    key = (grant["context"], grant["member"])
                    with self.recovery_lock:
                        if self.recovery_holds.get(key) is hold:
                            del self.recovery_holds[key]
            except Exception as error:  # a lost report must never reach a delivery path
                self.event("wake_report_failed", member=grant["member"],
                           reason=type(error).__name__)
            finally:
                self.wake_reports.task_done()

    def flush_wake_reports(self, timeout):
        deadline = time.monotonic() + timeout
        while self.wake_reports.unfinished_tasks:
            if time.monotonic() > deadline:
                return False
            time.sleep(0.005)
        return True

    def rang(self, session, batch, written, recovery, attempted_us):
        binding = self.bindings.get(session)
        sequences = [item.get("sequence") for item in batch.get("items") or []
                     if isinstance(item, dict)]
        sequences = [value for value in sequences if type(value) is int and value > 0]
        if binding and sequences:
            self.report_wake(binding, max(sequences), "claude_channel",
                             "accepted" if written else "failed",
                             None if written else "channel_write_failed", recovery, attempted_us)

    def recover_native(self, session):
        """The next wait answers pending once even if the cursor has not moved."""
        with self.native_ready:
            self.native_recover.add(session)
        self.wake_native()

    def recover_once(self):
        """Re-hint each binding the core flags as overdue. Returns how many were re-hinted.

        Codex recovery shares QueueDelivery's lock with the feed worker; a forced channel ring
        is the same call a hello makes; a native mod gets a one-shot flag. None of them adds a
        second delivery path."""
        with self.registry_lock:
            candidates = [(session, binding, self.targets.get(session))
                          for session, binding in self.all_bindings.items()
                          if session not in self.quarantined]
        count = 0
        for session, binding, host in candidates:
            if self.feed.stopped.is_set():
                break
            status = self.read_wake_status(binding, 2.0)
            first = status.get("first_unread_sequence")
            if (status.get("type") != "binding_status" or status.get("recovery_due") is not True
                    or type(first) is not int or first < 1):
                continue
            key = (binding["context"], binding["member"])
            hold = {"first": first, "reported": False}
            with self.recovery_lock:
                held = self.recovery_holds.get(key, {}).get("first") == first
                if not held:
                    self.recovery_holds[key] = hold  # before the wake, so an early ack finds it
            if held:
                self.event("recovery_unacknowledged", member=binding["member"])
                continue
            hint = {"context": binding["context"], "member": binding["member"], "sequence": first}
            done = False
            try:
                if host == "codex":
                    with self.registry_lock:
                        callback = self.queued.get(session)
                    done = callback is not None and callback.recover(hint) is True
                elif host == "claude":
                    is_native, channel_missing = self.native_notification_mode(session)
                    if is_native:
                        # The report comes when the mod's next wait answers; the hold waits.
                        with self.recovery_lock:
                            hold["reported"] = True
                        self.recover_native(session)
                        done = True
                    elif not channel_missing:
                        done = self.notify(session, force=True,
                                           recovery=True).get("_notified") is True
            finally:
                with self.recovery_lock:
                    if self.recovery_holds.get(key) is hold and not hold["reported"]:
                        del self.recovery_holds[key]  # nothing was attempted, nothing to count
            if done:
                count += 1
            else:
                self.event("recovery_skipped", member=binding["member"])
        return count

    def recovery_loop(self):
        while not self.feed.stopped.wait(self.recovery_interval):
            try:
                self.recover_once()
            except Exception as error:
                self.event("recovery_failed", reason=type(error).__name__)

    def event(self, kind, level="info", **fields):
        # The one watch already emits this after durable changes. Waking HTTP
        # waiters requires no second watch, per-room timer, or model turn.
        if kind == "feed_changed" and hasattr(self, "native_ready"):
            self.wake_native()
        super().event(kind, level, **fields)

    def wake_native(self):
        with self.native_ready:
            self.native_generation += 1
            self.native_ready.notify_all()

    def deliver_hint(self, session):
        with self.native_ready:
            is_native = session in self.native_sessions
        if is_native:
            # binding_status in the waiter is non-consuming. notify() would call
            # check and freeze a batch before the native host asks for its inbox.
            self.wake_native()
            return True
        return super().deliver_hint(session)

    def native_notification_mode(self, session):
        with self.native_ready:
            is_native = session in self.native_sessions
        with self.lock:
            channel_missing = session in self.channel_sessions and session not in self.channels
        return is_native, channel_missing

    def notify(self, session, force=False, recovery=False):
        """Native wake and unconnected channels must never prefetch an inbox batch."""
        is_native, channel_missing = self.native_notification_mode(session)
        if not is_native and not channel_missing:
            return super().notify(session, force=force, recovery=recovery)
        if not is_native and not force:
            return {"type": "skipped"}
        binding = self.native_binding(session)
        if not binding:
            return {"type": "error", "message": "invalid binding"}
        if is_native:
            self.wake_native()
        reply = self.wake_status(binding, 2.0)
        if reply.get("type") != "binding_status":
            return reply
        first = reply.get("first_unread_sequence")
        if ("first_unread_sequence" not in reply
                or first is not None and (type(first) is not int or first < 1)):
            return {"type": "error", "message": "invalid binding status"}
        return reply

    def handle_request(self, conn, pid, uid, raw=None):
        raw = claude.read_frame(conn) if raw is None else raw
        try:
            request = json.loads(raw) if raw else None
        except ValueError:
            request = None
        if isinstance(request, dict) and request.get("op") == "hello":
            session = claude.corroborate(pid, uid, self.sessions_dir)
            binding = self.native_binding(session)
            if session and binding and any(self.native_notification_mode(session)):
                if not self.target_allowed(session, pid, uid):
                    return {"error": "native execution target mismatch"}
                if request != {"v": 1, "op": "hello"} or type(request.get("v")) is not int:
                    return {"error": "invalid request"}
                reply = self.notify(session, force=True)
                kind = reply.get("type")
                self.event("hello", member=binding["member"],
                           pending=kind == "binding_status" and reply["first_unread_sequence"] is not None,
                           outcome=kind)
                if kind != "binding_status":
                    return {"error": str(reply.get("message", "core error"))}
                return {"ok": True}
        # The existing MCP parser, identity checks, receipts and connected-channel
        # path retain their original response and error handling.
        return super().handle_request(conn, pid, uid, raw=raw)

    def native_identity(self, pid, uid):
        session = native.corroborate_native(pid, uid, self.sessions_dir,
                                           claude.procinfo, claude.record_start)
        if session and self.target_allowed(session, pid, uid, direct=True):
            return session
        return None

    def native_binding(self, session):
        with self.registry_lock:
            if session in self.quarantined or self.targets.get(session) != "claude":
                return None
            return self.all_bindings.get(session)

    def verify_target_registry(self, bindings, targets):
        """Returns definitively revoked sessions; any other disagreement fails closed."""
        revoked = set()
        for session, binding in bindings.items():
            status = self.read_wake_status(binding, 2.0)
            if claude.native_target.is_revoked(status):
                revoked.add(session)
            elif not claude.native_target.target_agrees(targets[session]["target"], status):
                raise ValueError("native target disagrees with core binding authority")
        return revoked

    def wake_status(self, binding, timeout):
        status = self.read_wake_status(binding, timeout)
        with self.registry_lock:
            expected = self.native_targets.get(binding["session"])
            current = self.all_bindings.get(binding["session"])
        if (current != binding or expected is None
                or not claude.native_target.target_agrees(expected["target"], status)):
            if status.get("type") == "error":
                return status
            return {"type": "error", "code": "native_target_mismatch"}
        return status

    def read_wake_status(self, binding, timeout):
        """A bounded non-consuming core call, independent of MCP's long timeout."""
        return self.bounded_call(binding, {"op": "binding_status"}, timeout)

    def bounded_call(self, binding, request, timeout):
        return self.observed(binding, request, self.bounded_exchange(binding, request, timeout))

    def bounded_exchange(self, binding, request, timeout):
        deadline = time.monotonic() + timeout
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(timeout)
            try:
                connection.connect(self.core)
                connection.settimeout(max(0.001, deadline - time.monotonic()))
                connection.sendall((claude.dumps({"v": 1, "auth": binding,
                    "request": request}) + "\n").encode())
                raw = b""
                while not raw.endswith(b"\n"):
                    remaining = deadline - time.monotonic()
                    if remaining <= 0 or len(raw) >= claude.MAX_FRAME:
                        return {"type": "error"}
                    connection.settimeout(remaining)
                    part = connection.recv(min(65536, claude.MAX_FRAME + 1 - len(raw)))
                    if not part:
                        return {"type": "error"}
                    raw += part
                reply = json.loads(raw) if len(raw) <= claude.MAX_FRAME else None
                return reply if isinstance(reply, dict) else {"type": "error"}
            except (OSError, ValueError):
                return {"type": "error"}

    def handle_native(self, conn, pid, uid, initial, wake=None):
        try:
            after = native.read_request(conn, initial)
        except (OSError, ValueError):
            return native.response(400)
        if conn.family != socket.AF_UNIX:
            return native.response(401)
        session = self.native_identity(pid, uid)
        binding = self.native_binding(session)
        if not session or not binding:
            return native.response(401)
        with self.native_ready:
            if session in self.native_pending or not self.native_slots.acquire(blocking=False):
                return native.response(429)
            self.native_pending.add(session)
            attached = session not in self.native_sessions
            self.native_sessions.add(session)
        if attached:
            self.event("native_mod_attached", member=binding["member"])
        try:
            return self.wait_native(session, binding, pid, uid, after, wake)
        finally:
            with self.native_ready:
                self.native_pending.discard(session)
            self.native_slots.release()

    def wait_native(self, session, binding, pid, uid, after, wake=None):
        deadline = time.monotonic() + native.WAIT_SECONDS
        while True:
            with self.native_ready:
                generation = self.native_generation
            if self.feed.stopped.is_set():
                return native.response(503)
            if (self.native_binding(session) != binding
                    or self.native_identity(pid, uid) != session):
                return native.response(401)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return native.response(204)
            reply = self.wake_status(binding, min(2.0, remaining))
            if reply.get("type") == "error" and reply.get("code") in ("invalid_binding", "native_target_mismatch"):
                return native.response(401)
            if reply.get("type") != "binding_status" or "first_unread_sequence" not in reply:
                return native.response(503)
            first = reply.get("first_unread_sequence")
            if first is not None and (type(first) is not int or first < 1):
                return native.response(503)
            token = native.cursor(binding, first) if first is not None else None
            with self.native_ready:
                recovering = session in self.native_recover
                if token is None:
                    self.native_recover.discard(session)
            if token is not None and (token != after or recovering):
                # Revalidate both native process identity and binding authority
                # after I/O. Never deliver an event for a replaced/revoked chat.
                if (self.native_identity(pid, uid) != session
                        or self.native_binding(session) != binding):
                    return native.response(401)
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return native.response(503)
                fresh = self.wake_status(binding, min(2.0, remaining))
                if fresh.get("type") == "error" and fresh.get("code") in ("invalid_binding", "native_target_mismatch"):
                    return native.response(401)
                if fresh.get("type") != "binding_status" or "first_unread_sequence" not in fresh:
                    return native.response(503)
                fresh_first = fresh["first_unread_sequence"]
                if fresh_first is not None and (type(fresh_first) is not int or fresh_first < 1):
                    return native.response(503)
                if fresh.get("first_unread_sequence") != first:
                    continue
                if (self.native_identity(pid, uid) != session
                        or self.native_binding(session) != binding):
                    return native.response(401)
                # Read outside native_ready: dispatch emits feed_changed (which takes
                # native_ready) while holding the feed's lock.
                hinted = self.feed.hinted((binding["context"], binding["member"]))
                with self.native_ready:
                    recovery = session in self.native_recover
                    self.native_recover.discard(session)
                    through = max(fresh_first, self.latest_hint.get(session, 0), hinted)
                self.event("native_mod_pending", member=binding["member"])
                # The attempt time is taken before the 200 is written, so it precedes the mod
                # hearing of it; serve reports the outcome once the write has succeeded or failed.
                if wake is not None:
                    wake.append((binding, through, recovery, time.time_ns() // 1000))
                # A recovery is marked as such: the mod drops a repeated pending cursor, and a
                # recovery often has one. (Codex review 7.)
                return native.response(200, {"v": 1, "type": "recovery" if recovery else "pending",
                                             "cursor": token})
            with self.native_ready:
                if generation != self.native_generation:
                    continue
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return native.response(204)
                self.native_ready.wait(remaining)
                timed_out = generation == self.native_generation
            if timed_out:
                # No registry lock while holding native_ready: refresh acquires
                # them in the opposite order. Heartbeats perform no core poll.
                if self.feed.stopped.is_set():
                    return native.response(503)
                if (self.native_identity(pid, uid) != session
                        or self.native_binding(session) != binding):
                    return native.response(401)
                return native.response(204)

    def serve(self, conn):
        keep = False
        try:
            conn.settimeout(native.HEADER_SECONDS)
            initial = conn.recv(1)
            if not initial:
                return
            pid, uid = claude.peer_credentials(conn)
            if initial not in b"{[ \t\r\n":
                wake = []
                reply = self.handle_native(conn, pid, uid, initial, wake)
                failure = "mod_write_failed"
                try:
                    conn.settimeout(1)
                    conn.sendall(reply)
                    failure = None
                finally:
                    # Acceptance is the pending reaching the mod. (Codex review 6.)
                    for binding, through, recovery, attempted_us in wake:
                        self.report_wake(binding, through, "claude_mod",
                                         "failed" if failure else "accepted", failure,
                                         recovery, attempted_us)
                return
            # Preserve the original raw newline JSON protocol and channel ownership.
            conn.settimeout(10)
            raw = initial
            while not raw.endswith(b"\n") and len(raw) <= claude.MAX_FRAME:
                chunk = conn.recv(min(65536, claude.MAX_FRAME + 1 - len(raw)))
                if not chunk:
                    break
                raw += chunk
            if len(raw) > claude.MAX_FRAME or not raw.endswith(b"\n"):
                reply = {"error": "invalid request"}
            else:
                reply = self.handle(conn, raw=raw, credentials=(pid, uid))
            if reply is claude.SUBSCRIBED:
                keep = True
            elif reply is not None:
                conn.sendall((claude.dumps(reply) + "\n").encode())
        except OSError:
            pass
        finally:
            if not keep:
                self.drop_channel(conn)
            self.slots.release()

    def install_registry(self):
        """Caller holds registry_lock; one shared feed survives membership changes."""
        active = {s: b for s, b in self.all_bindings.items() if s not in self.quarantined}
        with self.lock:
            self.bindings = {s: b for s, b in active.items() if self.targets[s] == "claude"}
            self.channel_sessions = set(self.bindings)
        handlers = {(b["context"], b["member"]): self.handlers[(b["context"], b["member"])]
                    for b in active.values()}
        self.feed.replace_registry(active.values(), handlers)
        with self.native_ready:
            self.native_sessions.intersection_update(self.bindings)
        self.wake_native()

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
        core, bindings, targets, native_targets = load_targets(self.profile_path, with_native=True)
        if core != self.core:
            raise ValueError("profile refresh cannot change the core endpoint")
        revoked = self.verify_target_registry(bindings, native_targets)
        queued, handlers = {}, {}
        with self.registry_lock:
            for session, binding in bindings.items():
                key = (binding["context"], binding["member"])
                if targets[session] == "claude":
                    handlers[key] = self.claude_handler(session)
                else:
                    if not self.codex_executable:
                        raise ValueError("Codex executable missing")
                    prior = self.queued.get(session)
                    callback = (prior if prior and self.all_bindings.get(session) == binding
                                else codex.QueueDelivery(self.codex_executable, session, self.event))
                    callback.status_provider = lambda binding=binding: self.core_call(binding, {"op": "binding_status"})
                    callback.reporter = self.codex_reporter(binding)
                    queued[session] = handlers[key] = callback
            self.all_bindings, self.targets, self.queued, self.handlers = bindings, targets, queued, handlers
            self.native_targets = native_targets
            self.quarantined = set(revoked)
            self.install_registry()
        if revoked:
            self.event("bindings_quarantined", count=len(revoked))
        self.event("profile_refreshed", targets=len(bindings))

    def handle(self, conn, raw=None, credentials=None):
        pid, uid = credentials or claude.peer_credentials(conn)
        raw = claude.read_frame(conn) if raw is None else raw
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
    recovery = threading.Thread(target=router.recovery_loop, name="cbc-wake-recovery")

    def stop(*_args):
        stopped.set()
        router.feed.stop()
        router.wake_native()
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
    recovery.start()
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
                    connection.settimeout(0.1)
                    try:
                        connection.sendall(native.response(429))
                    except OSError:
                        pass
                    connection.close()
    finally:
        stop()
        follower.join()
        recovery.join()
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
    parser.add_argument("--run", type=claude.event_log.run_id,
                        help="trial run id written on every log line")
    args = parser.parse_args()
    try:
        router = DeviceRouter(args)
        run(router, args.listen)
    except (OSError, ValueError) as error:
        parser.exit(1, f"device router unavailable: {type(error).__name__}\n")


if __name__ == "__main__":
    main()
