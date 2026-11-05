"""Host-neutral routing tests. No native app, profile, daemon or model calls."""
import importlib.util
import pathlib
import threading
import unittest

spec = importlib.util.spec_from_file_location("feed", pathlib.Path(__file__).with_name("feed.py"))
feed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(feed)


def grant(member, context="ctx"):
    return {"context": context, "member": member, "device": "fixture", "session": "native-" + member,
            "epoch": 1, "token": "not-a-real-token"}


def changed(*members):
    return {"type": "members_changed", "sequence": max(m["sequence"] for m in members), "members": list(members)}


class RoutingTests(unittest.TestCase):
    def make_feed(self):
        self.calls = []
        self.events = []
        # Same member slug in two contexts must select different host handlers.
        handlers = {("codex", "lead"): lambda hint: self.calls.append(("codex", hint)),
                    ("claude", "lead"): lambda hint: self.calls.append(("claude", hint))}
        return feed.ChangeFeed("/unused", [grant("lead", "codex"), grant("lead", "claude")], handlers,
                               lambda kind, **fields: self.events.append((kind, fields)))

    def test_routes_only_changed_targets_to_their_bound_host(self):
        router = self.make_feed()
        hint = {"context": "claude", "member": "lead", "sequence": 4}
        self.assertEqual(router.dispatch(changed(hint), 0), 4)
        router.deliver_ready()
        self.assertEqual(self.calls, [("claude", hint)])
        router.dispatch(changed({"context": "codex", "member": "lead", "sequence": 5}), 4)
        router.deliver_ready()
        self.assertEqual([host for host, _ in self.calls], ["claude", "codex"])

    def test_validates_entire_frame_before_any_host_is_called(self):
        valid = {"context": "codex", "member": "lead", "sequence": 1}
        invalid = [
            {"context": "other", "member": "lead", "sequence": 2},
            {"context": "claude", "member": "lead", "sequence": True},
            {"context": "claude", "member": "lead", "sequence": 2, "body": "not a hint"},
            valid,
        ]
        for bad in invalid:
            router = self.make_feed()
            with self.assertRaises(ValueError):
                router.dispatch(changed(valid, bad), 0)
            self.assertEqual(self.calls, [])

    def test_stale_empty_or_inconsistent_responses_are_not_empty_success(self):
        hint = {"context": "codex", "member": "lead", "sequence": 1}
        replies = [changed(hint), {"type": "members_changed", "sequence": 2, "members": [hint]},
                   {"type": "members_changed", "sequence": 2, "members": []},
                   {"type": "error", "message": "core unavailable"}]
        for reply in replies:
            router = self.make_feed()
            with self.assertRaises(ValueError):
                router.dispatch(reply, 1)
            self.assertEqual(self.calls, [])

    def test_a_failing_host_cannot_suppress_another_host(self):
        router = self.make_feed()
        def fail(_hint):
            raise OSError("private internal detail must not be logged")
        router.handlers[("codex", "lead")] = fail
        router.dispatch(changed({"context": "codex", "member": "lead", "sequence": 1},
                                {"context": "claude", "member": "lead", "sequence": 2}), 0)
        while router.deliver_ready():
            pass
        self.assertEqual([host for host, _ in self.calls], ["claude"])
        self.assertTrue(any(kind == "target_failed" and fields["reason"] == "OSError"
                            for kind, fields in self.events))
        self.assertNotIn("private internal", str(self.events))

    def test_duplicate_members_and_missing_handlers_fail_before_connecting(self):
        with self.assertRaises(ValueError):
            feed.ChangeFeed("/unused", [grant("a"), grant("a")], {("ctx", "a"): lambda _: None})
        with self.assertRaises(ValueError):
            feed.ChangeFeed("/unused", [grant("a")], {})

    def test_failed_delivery_retries_without_a_new_core_message_and_coalesces_updates(self):
        now, attempts = [0.0], []
        def deliver(hint):
            attempts.append(hint["sequence"])
            return len(attempts) > 1
        router = feed.ChangeFeed("/unused", [grant("a")], {("ctx", "a"): deliver}, clock=lambda: now[0])
        router.dispatch(changed({"context": "ctx", "member": "a", "sequence": 1}), 0)
        self.assertTrue(router.deliver_ready())
        router.dispatch(changed({"context": "ctx", "member": "a", "sequence": 2}), 1)
        self.assertFalse(router.deliver_ready(), "new messages must not defeat failure backoff")
        now[0] = 1.0
        self.assertTrue(router.deliver_ready())
        self.assertEqual(attempts, [1, 2])
        self.assertEqual(router.pending_count(), 0)
        now[0] = 1000.0
        self.assertFalse(router.deliver_ready(), "success must not create idle retry ticks")

    def test_a_slow_host_does_not_block_the_other_shared_worker(self):
        blocked, release, progressed = threading.Event(), threading.Event(), threading.Event()
        class ParkedFeed(feed.ChangeFeed):
            def watch(self, _after):
                self.stopped.wait()
                raise OSError("stopped")
        def slow(_hint):
            blocked.set()
            release.wait(5)
        router = ParkedFeed("/unused", [grant("a"), grant("b")], {
            ("ctx", "a"): slow, ("ctx", "b"): lambda _: progressed.set()})
        thread = threading.Thread(target=router.run)
        thread.start()
        producer = threading.Thread(target=lambda: router.dispatch(changed(
            {"context": "ctx", "member": "a", "sequence": 1},
            {"context": "ctx", "member": "b", "sequence": 2}), 0))
        try:
            producer.start()
            self.assertTrue(blocked.wait(1))
            self.assertTrue(progressed.wait(1), "a slow host serialized the entire device")
        finally:
            release.set()
            producer.join(2)
            router.stop()
            thread.join(2)
        self.assertFalse(thread.is_alive())

    def test_live_retry_policy_applies_without_restart_and_retains_last_valid_values(self):
        now, lookups, attempts = [0.0], [], []
        policy = {"retry_initial_ms": 100, "retry_max_ms": 500}
        def read_policy(key):
            lookups.append(key)
            return dict(policy)
        def deliver(hint):
            attempts.append(hint["sequence"])
            return False
        router = feed.ChangeFeed("/unused", [grant("a")], {("ctx", "a"): deliver},
                                 clock=lambda: now[0], policy_provider=read_policy)
        self.assertFalse(router.deliver_ready())
        self.assertEqual(lookups, [], "idle operation must not query configuration")
        router.dispatch(changed({"context": "ctx", "member": "a", "sequence": 1}), 0)
        router.deliver_ready()
        self.assertAlmostEqual(router.pending[("ctx", "a")]["due"], .1)
        policy.update(retry_initial_ms=200, retry_max_ms=1000)
        now[0] = .1
        router.deliver_ready()
        self.assertAlmostEqual(router.pending[("ctx", "a")]["due"], .5)
        policy.update(retry_initial_ms=True)  # bool is not an integer policy value.
        now[0] = .5
        router.deliver_ready()
        self.assertAlmostEqual(router.pending[("ctx", "a")]["due"], 1.3)
        self.assertEqual(len(attempts), 3)
        self.assertEqual(lookups, [("ctx", "a")] * 3)

    def test_policy_failure_does_not_expose_data_or_create_successful_idle_work(self):
        events = []
        def unavailable(_key):
            raise OSError("secret profile detail")
        router = feed.ChangeFeed("/unused", [grant("a")], {("ctx", "a"): lambda _: True},
            lambda kind, **fields: events.append((kind, fields)), policy_provider=unavailable)
        self.assertEqual(router._retry_delay(1), 1.0)
        self.assertNotIn("secret profile", str(events))
        router.dispatch(changed({"context": "ctx", "member": "a", "sequence": 1}), 0)
        router.deliver_ready()
        self.assertEqual(router.pending_count(), 0)
        self.assertFalse(router.deliver_ready())

    def test_registry_replacement_drops_removed_work_and_rejects_stale_frames(self):
        router = self.make_feed()
        router.dispatch(changed({"context": "codex", "member": "lead", "sequence": 1},
                                {"context": "claude", "member": "lead", "sequence": 2}), 0)
        router.replace_registry([grant("lead", "claude")], {("claude", "lead"): router.handlers[("claude", "lead")]})
        self.assertEqual(router.pending_count(), 1)
        self.assertTrue(router.deliver_ready())
        self.assertEqual([host for host, _ in self.calls], ["claude"])
        with self.assertRaises(ValueError):
            router.dispatch(changed({"context": "codex", "member": "lead", "sequence": 3}), 0)

    def test_inflight_completion_cannot_remove_or_mutate_rebound_work(self):
        entered, release = threading.Event(), threading.Event()
        def blocked(_hint):
            entered.set()
            release.wait(3)
            return False
        first = grant("a")
        router = feed.ChangeFeed("/unused", [first], {("ctx", "a"): blocked})
        router.dispatch(changed({"context": "ctx", "member": "a", "sequence": 1}), 0)
        thread = threading.Thread(target=router.deliver_ready)
        thread.start()
        try:
            self.assertTrue(entered.wait(1))
            router.replace_registry([first | {"epoch": 2}], {("ctx", "a"): lambda _: True})
            router.dispatch(changed({"context": "ctx", "member": "a", "sequence": 2}), 0)
            current = router.pending[("ctx", "a")]
        finally:
            release.set()
            thread.join(2)
        self.assertIs(router.pending[("ctx", "a")], current)
        self.assertEqual(current["attempts"], 0)
        self.assertTrue(router.deliver_ready())

    def test_empty_quarantine_parks_without_watch_or_retry_until_explicit_refresh(self):
        watched = threading.Event()
        class ParkedFeed(feed.ChangeFeed):
            def watch(self, _after):
                watched.set()
                self.stopped.wait()
                raise OSError("stopped")
        router = ParkedFeed("/unused", [grant("a")], {("ctx", "a"): lambda _: True})
        router.replace_registry([], {})
        thread = threading.Thread(target=router.run)
        thread.start()
        try:
            self.assertFalse(watched.wait(.05))
            router.replace_registry([grant("b")], {("ctx", "b"): lambda _: True})
            self.assertTrue(watched.wait(1))
        finally:
            router.stop()
            thread.join(2)
        self.assertFalse(thread.is_alive())

    def test_explicit_registry_refresh_interrupts_long_failure_backoff(self):
        failed, resumed = threading.Event(), threading.Event()
        attempts = []
        class FailureFeed(feed.ChangeFeed):
            def watch(self, after):
                attempts.append(after)
                if len(attempts) == 1:
                    raise OSError("fixture outage")
                resumed.set()
                self.stopped.wait()
                raise OSError("stopped")
        router = FailureFeed("/unused", [grant("a")], {("ctx", "a"): lambda _: True},
            lambda kind, **fields: failed.set() if kind == "feed_failed" else None,
            policy_provider=lambda _: {"retry_initial_ms": 300000, "retry_max_ms": 300000})
        thread = threading.Thread(target=router.run)
        thread.start()
        try:
            self.assertTrue(failed.wait(1))
            router.replace_registry([grant("b")], {("ctx", "b"): lambda _: True})
            self.assertTrue(resumed.wait(1), "profile lifecycle refresh slept through the retry deadline")
            self.assertEqual(attempts, [0, 0])
        finally:
            router.stop()
            thread.join(2)


if __name__ == "__main__":
    unittest.main()
