"""One content-free core subscription, with host-specific delivery kept behind callbacks.

This module runs inside the device router, never as a process per chat or app. Callbacks
receive only a routing hint, not credentials, message bodies or permission authority.
Native delivery remains each host adapter's responsibility; a hint is not a receipt.
"""
import json
import socket
import threading
import time

MAX_FRAME = 256 * 1024
MAX_BINDINGS = 64
AUTH_FIELDS = {"context", "member", "device", "session", "epoch", "token"}


class ChangeFeed:
    def __init__(self, core, bindings, handlers, event=None, *, clock=time.monotonic,
                 policy_provider=None, reconcile=None):
        self.core = core
        self.bindings = list(bindings)
        if not 1 <= len(self.bindings) <= MAX_BINDINGS:
            raise ValueError("invalid subscription count")
        self.keys = set()
        for binding in self.bindings:
            if not isinstance(binding, dict) or set(binding) != AUTH_FIELDS:
                raise ValueError("invalid binding")
            key = (binding["context"], binding["member"])
            if key in self.keys:
                raise ValueError("duplicate subscribed member")
            self.keys.add(key)
        if set(handlers) != self.keys or not all(callable(h) for h in handlers.values()):
            raise ValueError("each subscribed member needs one delivery handler")
        self.handlers = dict(handlers)
        self.event = event or (lambda *_args, **_kwargs: None)
        self.stopped = threading.Event()
        self.lock = threading.Lock()
        self.connection = None
        self.clock = clock
        self.ready = threading.Condition()
        self.pending = {}
        self.started = False
        self.registry_revision = 0
        self.registry_changed = threading.Event()
        if policy_provider is not None and not callable(policy_provider):
            raise ValueError("retry policy provider must be callable")
        self.policy_provider = policy_provider
        self.policies = {}
        if reconcile is not None and not callable(reconcile):
            raise ValueError("binding reconciler must be callable")
        self.reconcile = reconcile

    def replace_registry(self, bindings, handlers):
        """Explicit lifecycle refresh, including an empty quarantined registry. No timer."""
        bindings = list(bindings)
        if len(bindings) > MAX_BINDINGS:
            raise ValueError("invalid subscription count")
        keys = []
        for binding in bindings:
            if not isinstance(binding, dict) or set(binding) != AUTH_FIELDS:
                raise ValueError("invalid binding")
            keys.append((binding["context"], binding["member"]))
        if (len(set(keys)) != len(keys) or set(handlers) != set(keys)
                or not all(callable(handler) for handler in handlers.values())):
            raise ValueError("invalid refreshed handlers")
        with self.ready:
            with self.lock:
                old = {(b["context"], b["member"]): b for b in self.bindings}
                fresh = dict(zip(keys, bindings))
                self.bindings, self.keys, self.handlers = bindings, set(keys), dict(handlers)
                self.registry_revision += 1
                self.registry_changed.set()
                self.pending = {key: state for key, state in self.pending.items()
                                if key in fresh and old.get(key) == fresh[key]}
                if self.connection is not None:
                    try:
                        self.connection.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
            self.ready.notify_all()

    def _retry_delay(self, attempts, key=None):
        """Read live policy only when retrying real work; preserve last valid values."""
        with self.lock:
            policy = self.policies.get(key, (1000, 30000))
        if self.policy_provider is not None:
            try:
                current = self.policy_provider(key)
                if not isinstance(current, dict):
                    raise ValueError("invalid retry policy")
                initial, maximum = current.get("retry_initial_ms"), current.get("retry_max_ms")
                if (type(initial) is not int or type(maximum) is not int
                        or not 10 <= initial <= maximum <= 300000):
                    raise ValueError("invalid retry policy")
                policy = initial, maximum
                with self.lock:
                    self.policies[key] = policy
            except Exception as error:
                self.emit("settings_failed", reason=type(error).__name__)
        return min(policy[1], policy[0] * 2 ** min(max(attempts - 1, 0), 20)) / 1000.0

    def emit(self, kind, **fields):
        # Diagnostics are not part of the delivery path and contain no frame/profile data.
        try:
            self.event(kind, **fields)
        except Exception:
            pass

    def stop(self):
        self.stopped.set()
        self.registry_changed.set()
        with self.ready:
            self.ready.notify_all()
        with self.lock:
            if self.connection is not None:
                try:
                    self.connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass

    def watch(self, after):
        connection = socket.socket(socket.AF_UNIX)
        with self.lock:
            if self.stopped.is_set():
                connection.close()
                raise OSError("feed stopped")
            self.connection = connection
            bindings = list(self.bindings)
            if self.registry_changed.is_set():
                after = 0
        try:
            if not bindings:
                raise ValueError("no active bindings")
            connection.settimeout(30)
            connection.connect(self.core)
            request = {"v": 1, "auth": bindings[0], "request": {
                "op": "watch_members", "others": bindings[1:], "after": after}}
            connection.sendall((json.dumps(request, ensure_ascii=False, separators=(",", ":")) + "\n").encode())
            connection.settimeout(None)  # No successful idle timer or model wake.
            with connection.makefile("rb") as stream:
                raw = stream.readline(MAX_FRAME + 1)
            if not raw.endswith(b"\n") or len(raw) > MAX_FRAME:
                raise ValueError("invalid feed frame")
            return json.loads(raw)
        finally:
            with self.lock:
                self.connection = None
            connection.close()

    def dispatch(self, reply, after):
        """Validate the whole hint, then coalesce it into the shared delivery queue."""
        with self.ready:
            return self._dispatch_registered(reply, after)

    def _dispatch_registered(self, reply, after):
        # Registry replacement and frame validation must observe the same generation.
        if (not isinstance(reply, dict) or set(reply) != {"type", "sequence", "members"}
                or reply["type"] != "members_changed"
                or type(reply["sequence"]) is not int or reply["sequence"] <= after
                or not isinstance(reply["members"], list)
                or not 1 <= len(reply["members"]) <= len(self.bindings)):
            raise ValueError("invalid targeted feed reply")
        changes, seen = [], set()
        for change in reply["members"]:
            if (not isinstance(change, dict) or set(change) != {"context", "member", "sequence"}
                    or not isinstance(change["context"], str) or not isinstance(change["member"], str)
                    or type(change["sequence"]) is not int
                    or not after < change["sequence"] <= reply["sequence"]):
                raise ValueError("invalid member hint")
            key = (change["context"], change["member"])
            if key not in self.keys or key in seen:
                raise ValueError("unregistered or duplicate member hint")
            seen.add(key)
            changes.append((key, change))
        if max(change["sequence"] for _, change in changes) != reply["sequence"]:
            raise ValueError("inconsistent feed sequence")
        with self.ready:
            if self.stopped.is_set():
                raise OSError("feed stopped")
            for key, change in changes:
                current = self.pending.get(key)
                if current is None:
                    self.pending[key] = {"hint": dict(change), "due": self.clock(),
                                         "attempts": 0, "inflight": False}
                elif change["sequence"] > current["hint"]["sequence"]:
                    # New activity must not defeat retry backoff or duplicate an in-flight call.
                    current["hint"] = dict(change)
            self.ready.notify_all()
        self.emit("feed_changed", sequence=reply["sequence"], targets=len(changes))
        return reply["sequence"]

    def hinted(self, key):
        """The highest sequence hinted for a member and not yet delivered, or 0.

        Set before feed_changed is emitted, so a waiter woken by that event can read it even
        though the delivery worker has not run the member's handler yet."""
        with self.ready:
            state = self.pending.get(key)
            return state["hint"]["sequence"] if state else 0

    def pending_count(self):
        with self.ready:
            return len(self.pending)

    def _take_ready(self):
        """Called under the condition lock; at most one invocation per member is in flight."""
        if self.stopped.is_set():
            return None
        candidates = [(state["due"], key) for key, state in self.pending.items() if not state["inflight"]]
        if not candidates:
            return None
        due, key = min(candidates)
        if due > self.clock():
            return None
        state = self.pending[key]
        state["inflight"] = True
        return key, dict(state["hint"]), state, self.handlers[key]

    def _deliver(self, key, hint, selected, handler):
        reason = None
        try:
            result = handler(dict(hint))
            if result is not None and type(result) is not bool:
                raise ValueError("delivery handler must return bool or None")
            if result is False:
                reason = "Unavailable"
        except Exception as error:
            reason = type(error).__name__
        # Provider I/O stays outside the condition lock so one host cannot freeze dispatch.
        if reason is not None:
            with self.ready:
                if self.pending.get(key) is not selected:
                    return
                attempts = selected["attempts"] + 1
            delay = self._retry_delay(attempts, key)
        with self.ready:
            state = self.pending.get(key)
            if state is not selected:
                return
            state["inflight"] = False
            if reason is None and state["hint"]["sequence"] == hint["sequence"]:
                del self.pending[key]
            elif reason is None:
                state.update(attempts=0, due=self.clock())
            else:
                state["attempts"] += 1
                state["due"] = self.clock() + delay
            self.ready.notify_all()
        if reason is not None:
            self.emit("target_failed", context=key[0], member=key[1], reason=reason, retry_in=delay)

    def deliver_ready(self):
        """Attempt one due target without waiting. Also used for deterministic clock tests."""
        with self.ready:
            work = self._take_ready()
        if work is None:
            return False
        self._deliver(*work)
        return True

    def _worker(self):
        while not self.stopped.is_set():
            with self.ready:
                if self.stopped.is_set():
                    return
                work = self._take_ready()
                if work is None:
                    due = [s["due"] for s in self.pending.values() if not s["inflight"]]
                    # No work means an unbounded wait, not an idle poll. Only failed
                    # delivery creates a deadline, shared by this bounded worker pool.
                    self.ready.wait(max(0, min(due) - self.clock()) if due else None)
                    continue
            self._deliver(*work)

    def run(self):
        with self.lock:
            if self.started:
                raise ValueError("feed already started; create a fresh instance for recovery")
            self.started = True
        # A fixed two-worker budget for the device, never a thread/process per chat.
        # Native handlers must remain bounded; Python cannot cancel their external effects.
        workers = [threading.Thread(target=self._worker, name="cbc-delivery") for _ in range(2)]
        started = []
        try:
            for worker in workers:
                worker.start()
                started.append(worker)
            self._follow()
        finally:
            self.stop()
            for worker in started:
                worker.join()

    def _follow(self):
        after, failures = 0, 0
        while not self.stopped.is_set():
            with self.ready:
                while not self.bindings and not self.stopped.is_set():
                    self.ready.wait()
                if self.stopped.is_set():
                    break
                generation = self.registry_revision
                self.registry_changed.clear()
            try:
                reply = self.watch(after)
                with self.ready:
                    if generation != self.registry_revision:
                        after, failures = 0, 0
                        continue
                    after = self.dispatch(reply, after)
                failures = 0
            except Exception as error:
                if self.stopped.is_set():
                    break
                with self.ready:
                    if generation != self.registry_revision:
                        after, failures = 0, 0
                        continue
                if self.reconcile is not None:
                    try:
                        if self.reconcile():
                            after, failures = 0, 0
                            continue
                    except Exception as reconcile_error:
                        self.emit("binding_reconcile_failed", reason=type(reconcile_error).__name__)
                # Recover missed hints from durable unread state on reconnect. Backoff is
                # failure-only and shared, not an idle poll or a timer per recipient.
                after = 0
                failures += 1
                delay = self._retry_delay(failures)
                self.emit("feed_failed", reply=type(error).__name__, retry_in=delay)
                self.registry_changed.wait(delay)
