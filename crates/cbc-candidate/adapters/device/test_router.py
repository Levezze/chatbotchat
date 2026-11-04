#!/usr/bin/env python3
"""Disposable driver contract tests. Fake host callbacks are not native-app evidence."""
import importlib.util
import json
from pathlib import Path
import os
import subprocess
import socket
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("device_router", HERE / "router.py")
device = importlib.util.module_from_spec(spec)
spec.loader.exec_module(device)
SESSION = "11111111-1111-4111-8111-111111111111"
CLAUDE = "22222222-2222-4222-8222-222222222222"
HINT = {"context": "project", "member": "codex", "sequence": 7}


def binding(member, session):
    return {"context": "project", "member": member, "device": "local", "session": session,
            "epoch": 1, "token": "cbcc1_" + "a" * 64}


class DeviceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cbc-device-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        os.chmod(self.root, 0o700)
        self.path = self.root / "device.json"
        self.profile = {"v": 1, "core_socket": str(self.root / "connector.sock"), "bindings": [
            binding("codex", SESSION), binding("claude", CLAUDE)], "targets": [
            {"session": SESSION, "host": "codex"}, {"session": CLAUDE, "host": "claude"}]}
        self.save()

    def save(self):
        self.path.write_text(json.dumps(self.profile))
        os.chmod(self.path, 0o600)

    def args(self):
        return SimpleNamespace(profile=str(self.path), sessions_dir=str(self.root / "sessions"),
                               log_dir=None, codex_executable="/usr/local/bin/codex")

    def cleanup_router(self, router):
        router.feed.stop()
        router.wake_read.close()
        router.wake_write.close()

    def test_private_profile_maps_every_native_session_to_one_host(self):
        core, bindings, targets = device.load_targets(str(self.path))
        self.assertEqual(core, self.profile["core_socket"])
        self.assertEqual(set(bindings), {SESSION, CLAUDE})
        self.assertEqual(targets, {SESSION: "codex", CLAUDE: "claude"})

    def test_valid_profile_above_64k_is_accepted_but_128k_bound_remains(self):
        self.path.write_text(json.dumps(self.profile) + " " * (70 * 1024))
        core, bindings, targets = device.load_targets(str(self.path))
        self.assertEqual(len(bindings), 2)
        self.assertEqual(device.claude.load_profile(str(self.path))[0], core)
        self.path.write_text(json.dumps(self.profile) + " " * (128 * 1024))
        with self.assertRaises(ValueError):
            device.load_targets(str(self.path))

    def test_unmapped_duplicate_and_malformed_profiles_fail_closed(self):
        original = json.loads(json.dumps(self.profile))
        for change in ("missing", "extra", "duplicate", "bad-binding", "unknown-key", "wrong-core"):
            self.profile = json.loads(json.dumps(original))
            if change == "missing":
                self.profile["targets"].pop()
            elif change == "extra":
                self.profile["targets"].append({"session": "other", "host": "codex"})
            elif change == "duplicate":
                self.profile["bindings"][1]["member"] = "codex"
            elif change == "bad-binding":
                self.profile["bindings"][0]["context"] = []
            elif change == "unknown-key":
                self.profile["arbitrary_command"] = "not allowed"
            else:
                self.profile["core_socket"] = "/private/tmp/unrelated.sock"
            self.save()
            with self.subTest(change=change), self.assertRaises(ValueError):
                device.load_targets(str(self.path))

    def test_profile_wrong_mode_and_symlink_are_refused(self):
        os.chmod(self.path, 0o644)
        with self.assertRaises(ValueError):
            device.load_targets(str(self.path))
        os.chmod(self.path, 0o600)
        linked = self.root / "linked.json"
        linked.symlink_to(self.path)
        with self.assertRaises((OSError, ValueError)):
            device.load_targets(str(linked))

    def test_fifo_profile_refuses_in_bounded_subprocess(self):
        path = self.root / "fifo-profile"
        os.mkfifo(path, 0o600)
        code = "import runpy,sys; runpy.run_path(sys.argv[1])[sys.argv[2]](sys.argv[3])"
        result = subprocess.run([sys.executable, "-B", "-I", "-c", code,
            str(HERE / "router.py"), "load_targets", str(path)],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=2)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"regular file", result.stderr)

    def test_one_feed_dispatches_only_the_changed_host(self):
        deliveries = []
        class FakeQueue:
            def __init__(self, executable, session, event=None):
                self.session = session
            def __call__(self, hint):
                deliveries.append(("codex", self.session, hint))
                return True
        with patch.object(device.codex, "QueueDelivery", FakeQueue):
            router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        router.deliver_hint = lambda session: deliveries.append(("claude", session)) or True
        self.assertEqual(set(router.bindings), {CLAUDE})
        self.assertEqual(len(router.feed.bindings), 2)
        reply = {"type": "members_changed", "sequence": 7, "members": [HINT]}
        router.feed.dispatch(reply, 0)
        self.assertTrue(router.feed.deliver_ready())
        self.assertEqual(deliveries, [("codex", SESSION, HINT)])
        router.feed.dispatch({"type": "members_changed", "sequence": 8, "members": [
            {"context": "project", "member": "claude", "sequence": 8}]}, 7)
        self.assertTrue(router.feed.deliver_ready())
        self.assertEqual(deliveries[-1], ("claude", CLAUDE))

    def test_claude_is_channel_only_before_subscribe_after_refresh_and_restart(self):
        def assert_no_fallback(router, session):
            self.assertIn(session, router.channel_sessions)
            self.assertNotIn(session, router.channels)
            router.core_call = lambda *_args: {"type": "batch", "reference": "fixture-reference", "items": []}
            with patch.object(router, "live_socket", side_effect=AssertionError("historical socket lookup")):
                self.assertEqual(router.notify(session), {"type": "skipped"})
                self.assertFalse(router.notify(session, force=True)["_notified"])
                self.assertFalse(router.ring(session, "/tmp/must-not-send-role-user.sock"))
        router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        assert_no_fallback(router, CLAUDE)
        replacement = "33333333-3333-4333-8333-333333333333"
        self.profile["bindings"][1]["session"] = replacement
        self.profile["targets"][1]["session"] = replacement
        self.save()
        router.refresh_profile()
        assert_no_fallback(router, replacement)
        restarted = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, restarted)
        assert_no_fallback(restarted, replacement)

    def test_settings_provider_is_bound_and_reads_only_when_called(self):
        router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        calls = []
        def core_call(grant, request):
            calls.append((grant["member"], request))
            return {"type": "settings", "revision": "revision-2", "policy": {
                "retry_initial_ms": 100, "retry_max_ms": 500}}
        router.core_call = core_call
        self.assertEqual(calls, [])
        policy = router.feed.policy_provider(("project", "claude"))
        self.assertEqual(calls, [("claude", {"op": "settings"})])
        self.assertEqual(policy["retry_initial_ms"], 100)
        self.assertEqual(policy["revision"], "revision-2")

    def test_no_codex_target_without_explicit_executable(self):
        args = self.args()
        args.codex_executable = None
        with self.assertRaises(ValueError):
            device.DeviceRouter(args)

    def test_device_socket_cannot_use_unrelated_or_core_path(self):
        router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        for path in (str(self.root / "connector.sock"), "/private/tmp/unrelated-device.sock", "relative.sock"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                device.run(router, path)

    def test_revocation_quarantines_only_definitive_invalid_binding(self):
        router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        calls = []
        def status(grant, request):
            calls.append((grant["member"], request))
            if grant["member"] == "codex":
                return {"type": "error", "code": "invalid_binding", "message": "revoked"}
            return {"type": "binding_status", "last_receipted_sequence": 0, "first_unread_sequence": None}
        router.core_call = status
        self.assertTrue(router.reconcile_bindings())
        self.assertEqual(router.quarantined, {SESSION})
        self.assertEqual(router.feed.keys, {("project", "claude")})
        self.assertEqual(set(router.bindings), {CLAUDE})
        self.assertFalse(router.reconcile_bindings())
        self.assertEqual(len(calls), 3)

    def test_core_outage_is_not_revocation_or_n_timeout_probes(self):
        router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        calls = []
        router.core_call = lambda grant, request: calls.append(grant) or {"type": "error", "message": "offline"}
        self.assertFalse(router.reconcile_bindings())
        self.assertEqual(len(calls), 1)
        self.assertEqual(router.quarantined, set())
        self.assertEqual(len(router.feed.bindings), 2)

    def test_explicit_profile_refresh_preserves_unchanged_latch_and_rebinds_changed_epoch(self):
        router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        original_feed, original_queue = router.feed, router.queued[SESSION]
        original_queue.accepted = (0, 1)
        router.quarantined.add(SESSION)
        router.install_registry()
        router.refresh_profile()
        self.assertIs(router.feed, original_feed)
        self.assertIs(router.queued[SESSION], original_queue)
        self.assertEqual(router.queued[SESSION].accepted, (0, 1))
        self.profile["bindings"][0]["epoch"] = 2
        self.save()
        router.refresh_profile()
        self.assertIsNot(router.queued[SESSION], original_queue)
        self.assertIsNone(router.queued[SESSION].accepted)
        self.assertEqual(router.feed.bindings[0]["epoch"], 2)
        prior = router.all_bindings
        self.profile["targets"].pop()
        self.save()
        with self.assertRaises(ValueError):
            router.refresh_profile()
        self.assertIs(router.all_bindings, prior, "invalid refresh must preserve the working registry")

    def test_receipt_nudge_uses_core_unread_facts_not_caller_sequences(self):
        router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        calls = []
        state = {"type": "binding_status", "last_receipted_sequence": 32, "first_unread_sequence": 33}
        router.core_call = lambda grant, request: calls.append((grant, request)) or state
        request = {"v": 1, "op": "codex_receipt", "auth": self.profile["bindings"][0]}
        self.assertEqual(router.receipt_nudge(request, os.getuid()), {"ok": True})
        self.assertEqual(router.feed.pending[("project", "codex")]["hint"]["sequence"], 33)
        self.assertEqual(calls[0][1], {"op": "binding_status"})
        router.queued[SESSION].accepted = (32, 33)
        state["first_unread_sequence"] = None
        self.assertEqual(router.receipt_nudge(request, os.getuid()), {"ok": True})
        self.assertIsNone(router.queued[SESSION].accepted)

    def test_receipt_nudge_refuses_forged_binding_wrong_uid_and_extra_fields(self):
        router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        router.core_call = lambda *_args: self.fail("untrusted nudge reached core")
        request = {"v": 1, "op": "codex_receipt", "auth": self.profile["bindings"][0]}
        invalid = [request | {"sequence": 99}, request | {"v": True},
                   request | {"auth": request["auth"] | {"token": "wrong"}},
                   request | {"auth": request["auth"] | {"session": []}},
                   request | {"auth": self.profile["bindings"][1]}]
        for value in invalid:
            self.assertIn("error", router.receipt_nudge(value, os.getuid()))
        self.assertIn("error", router.receipt_nudge(request, os.getuid() + 1))


class CodexQueueTests(unittest.TestCase):
    def test_repeated_and_newer_hints_coalesce_until_actual_receipt_progress(self):
        calls = []
        state = {"type": "binding_status", "last_receipted_sequence": 0, "first_unread_sequence": 1}
        callback = device.codex.QueueDelivery("/usr/local/bin/codex", SESSION,
            runner=lambda *args, **kwargs: calls.append(args) or SimpleNamespace(returncode=0),
            status_provider=lambda: dict(state))
        for sequence in (7, 7, 8, 100):
            self.assertTrue(callback(HINT | {"sequence": sequence}))
        self.assertEqual(len(calls), 1)
        state.update(last_receipted_sequence=32, first_unread_sequence=33)
        self.assertTrue(callback(HINT | {"sequence": 100}))
        self.assertEqual(len(calls), 2)
        state.update(last_receipted_sequence=100, first_unread_sequence=None)
        self.assertTrue(callback(HINT | {"sequence": 100}))
        self.assertEqual(len(calls), 2)
        self.assertIsNone(callback.accepted)
        state.update(first_unread_sequence=101)
        self.assertTrue(callback(HINT | {"sequence": 101}))
        self.assertEqual(len(calls), 3)

    def test_unavailable_status_does_not_unlatch_or_queue(self):
        calls = []
        callback = device.codex.QueueDelivery("/usr/local/bin/codex", SESSION,
            runner=lambda *args, **kwargs: calls.append(args) or SimpleNamespace(returncode=0),
            status_provider=lambda: {"type": "error", "code": "invalid_binding"})
        callback.accepted = (0, 1)
        self.assertFalse(callback(HINT))
        self.assertEqual(callback.accepted, (0, 1))
        self.assertEqual(calls, [])

    def test_queue_uses_only_target_uuid_and_content_free_hint(self):
        calls, events = [], []
        def runner(command, **kwargs):
            calls.append((command, kwargs))
            return SimpleNamespace(returncode=0)
        callback = device.codex.QueueDelivery("/usr/local/bin/codex", SESSION,
                                               lambda kind, **fields: events.append((kind, fields)),
                                               runner=runner)
        self.assertTrue(callback(HINT))
        command, kwargs = calls[0]
        self.assertEqual(command[:4], ["/usr/local/bin/codex", "queue", "--thread", SESSION])
        self.assertEqual(command[4], "--message")
        self.assertIn("cbc_check", command[5])
        self.assertNotIn("cbcc1_", command[5])
        self.assertNotIn("--resume", command)
        self.assertNotIn("--remote", command)
        self.assertEqual(kwargs["stdout"], subprocess.DEVNULL)
        self.assertEqual(kwargs["stderr"], subprocess.DEVNULL)
        self.assertNotIn("shell", kwargs)
        self.assertEqual(events, [("codex_queued", {"sequence": 7})])

    def test_unavailable_command_is_failure_not_fake_delivery(self):
        for outcome in (OSError("sensitive path"), subprocess.TimeoutExpired("private", 1), 1):
            events = []
            def runner(*_args, **_kwargs):
                if isinstance(outcome, Exception):
                    raise outcome
                return SimpleNamespace(returncode=outcome)
            callback = device.codex.QueueDelivery("/usr/local/bin/codex", SESSION,
                lambda kind, **fields: events.append((kind, fields)), runner=runner)
            self.assertFalse(callback(HINT))
            self.assertNotIn("sensitive", repr(events))
            self.assertNotIn("private", repr(events))

    def test_names_relative_commands_and_injected_payloads_are_refused(self):
        for session in ("Friendly name", "--all", "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA", "", None):
            with self.assertRaises(ValueError):
                device.codex.QueueDelivery("/usr/local/bin/codex", session)
        with self.assertRaises(ValueError):
            device.codex.QueueDelivery("codex", SESSION)
        callback = device.codex.QueueDelivery("/usr/local/bin/codex", SESSION)
        for hint in (HINT | {"body": "secret"}, HINT | {"sequence": True}, HINT | {"sequence": 0}):
            with self.assertRaises(ValueError):
                callback(hint)


if __name__ == "__main__":
    unittest.main()
