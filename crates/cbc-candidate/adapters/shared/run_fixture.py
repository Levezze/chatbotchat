"""Synthetic two-host dispatch through the real Rust core, invoked by its integration test.

Host callbacks are stand-ins, not evidence of Codex/Claude wake capability. The private
fixture profile is passed by the Rust test, never taken from installed host configuration.
"""
import importlib.util
import json
import pathlib
import queue
import socket
import sys
import threading

spec = importlib.util.spec_from_file_location("feed", pathlib.Path(__file__).with_name("feed.py"))
feed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(feed)


def main(profile_path):
    profile = json.loads(pathlib.Path(profile_path).read_text())
    socket_path = profile["core_socket"]
    bindings = {b["member"]: b for b in profile["bindings"]}

    def call(member, operation):
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(5)
            connection.connect(socket_path)
            request = {"v": 1, "auth": bindings[member], "request": operation}
            connection.sendall((json.dumps(request, separators=(",", ":")) + "\n").encode())
            with connection.makefile("rb") as stream:
                reply = json.loads(stream.readline(feed.MAX_FRAME + 1))
        if reply.get("type") == "error":
            raise RuntimeError("fixture core request rejected")
        return reply

    received = queue.Queue()
    waits = queue.Queue()
    attempts = {"codex": 0, "claude": 0}

    def deliver(host, hint):
        attempts[host] += 1
        if host == "codex" and attempts[host] == 1:
            return False  # A temporary native-adapter failure; no extra send will follow.
        received.put((host, hint))
        return True

    class CountingFeed(feed.ChangeFeed):
        def watch(self, after):
            waits.put(after)
            return super().watch(after)

    def start():
        handlers = {("fixture", host): lambda hint, host=host: deliver(host, hint)
                    for host in ("codex", "claude")}
        router = CountingFeed(socket_path, [bindings["codex"], bindings["claude"]], handlers)
        thread = threading.Thread(target=router.run)
        thread.start()
        assert waits.get(timeout=5) == 0
        return router, thread

    def stop(router, thread):
        router.stop()
        thread.join(timeout=5)
        assert not thread.is_alive(), "a parked feed did not shut down"

    router, thread = start()
    try:
        try:
            waits.get(timeout=0.1)
            raise AssertionError("idle feed reopened instead of parking")
        except queue.Empty:
            pass
        sequence = call("writer", {"op": "send", "room": "room", "key": "fixture-1",
                                   "body": "shared routing café ✓"})["sequence"]
        first = [received.get(timeout=5), received.get(timeout=5)]
        assert {host for host, _ in first} == {"codex", "claude"}
        assert attempts["codex"] == 2, "failed notification was lost or retried in a tight loop"
        assert all(hint == {"context": "fixture", "member": host, "sequence": sequence}
                   for host, hint in first)
        assert waits.get(timeout=5) == sequence
    finally:
        stop(router, thread)

    # Neither routing callback receipts a message. A fresh shared router recovers both.
    router, thread = start()
    try:
        replay = [received.get(timeout=5), received.get(timeout=5)]
        assert {host for host, _ in replay} == {"codex", "claude"}
        for host in ("codex", "claude"):
            batch = call(host, {"op": "check"})
            assert batch["type"] == "batch"
            assert batch["items"][0]["body"] == "shared routing café ✓"
            assert call(host, {"op": "receipt", "reference": batch["reference"]})["type"] == "receipted"
            assert call(host, {"op": "check"})["type"] == "empty"
    finally:
        stop(router, thread)
    print("shared router fixture passed: independent host callbacks, failed-delivery retry, parked idle, restart replay, explicit receipts")


if __name__ == "__main__":
    main(sys.argv[1])
