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
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch


HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("device_router", HERE / "router.py")
device = importlib.util.module_from_spec(spec)
spec.loader.exec_module(device)
VERIFY_TARGET_REGISTRY = device.DeviceRouter.verify_target_registry
SESSION = "11111111-1111-4111-8111-111111111111"
CLAUDE = "22222222-2222-4222-8222-222222222222"
HINT = {"context": "project", "member": "codex", "sequence": 7}


def binding(member, session):
    return {"context": "project", "member": member, "device": "local", "session": session,
            "epoch": 1, "token": "cbcc1_" + "a" * 64}


def execution_target(cwd, profile="unknown"):
    return {"v": 1, "profile": profile, "repository": "cbc-test", "worktree": None,
            "cwd": str(cwd), "saved_project": {"status": "not_exposed"},
            "saved_environment": {"status": "not_exposed"},
            "placement": {"evidence_ref": "synthetic-unit-evidence"}}


class DeviceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cbc-device-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        verified = patch.object(device.DeviceRouter, "verify_target_registry", return_value=set())
        verified.start()
        self.addCleanup(verified.stop)
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

    def test_v2_targets_are_required_and_strict_while_v1_remains_legacy(self):
        self.profile["v"] = 2
        for entry in self.profile["targets"]:
            entry["target"] = execution_target(self.root)
        self.save()
        core, bindings, hosts, native = device.load_targets(str(self.path), with_native=True)
        self.assertEqual(native[CLAUDE]["target"]["cwd"], str(self.root))
        self.assertEqual(device.claude.load_profile(str(self.path), with_targets=True)[2], native)
        original = json.loads(json.dumps(self.profile))
        for field, value in (("v", True), ("cwd", "/tmp/../wrong"), ("cwd", "/tmp//wrong"),
                             ("worktree", "/unrelated"), ("profile", "desktop-inferred"),
                             ("saved_project", {"status": "not_exposed", "id": "invented"}),
                             ("placement", {"evidence_ref": "x", "authority": True})):
            self.profile = json.loads(json.dumps(original))
            self.profile["targets"][1]["target"][field] = value
            self.save()
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                device.load_targets(str(self.path))
        self.profile = original
        del self.profile["targets"][1]["target"]
        self.save()
        with self.assertRaises(ValueError):
            device.load_targets(str(self.path))

    def test_kernel_cwd_observation_matches_current_process_and_fails_missing_pid(self):
        target = device.claude.native_target
        self.assertEqual(target.process_cwd(os.getpid()), os.path.realpath(os.getcwd()))
        self.assertIsNone(target.process_cwd(2147483647))

    def test_expected_paths_are_not_recanonicalized_or_placement_used_as_authority(self):
        target = device.claude.native_target
        actual = os.path.realpath(self.root)
        expected = execution_target(actual)
        observed = {"cwd": actual, "profile": "unknown", "saved_project": None,
                    "saved_environment": None}
        self.assertTrue(target.match_target(expected, observed))
        expected["placement"]["project_label"] = "Some desktop project"
        expected["profile"] = "desktop"
        self.assertFalse(target.match_target(expected, observed))
        expected["profile"] = "unknown"
        expected["saved_project"] = {"status": "configured", "id": "not-observable"}
        self.assertFalse(target.match_target(expected, observed))
        expected["saved_project"] = {"status": "not_exposed"}
        alias = self.root / "alias"
        alias.symlink_to(self.root, target_is_directory=True)
        expected["cwd"] = str(alias)
        self.assertFalse(target.match_target(expected, observed))

    def test_placement_audit_ids_roundtrip_without_satisfying_runtime_constraints(self):
        target = device.claude.native_target
        expected = execution_target(os.path.realpath(self.root))
        expected["placement"].update(project_id="owner-observed-project",
                                     environment_id="owner-observed-environment")
        observed = {"cwd": expected["cwd"], "profile": "unknown",
                    "saved_project": None, "saved_environment": None}
        self.assertEqual(target.validate_target(json.loads(json.dumps(expected))), expected)
        self.assertTrue(target.match_target(expected, observed))
        for selector, audit_key in (("saved_project", "project_id"),
                                   ("saved_environment", "environment_id")):
            expected[selector] = {"status": "configured", "id": expected["placement"][audit_key]}
            self.assertFalse(target.match_target(expected, observed))
            expected[selector] = {"status": "not_exposed"}
        for key in ("project_id", "environment_id"):
            original = expected["placement"][key]
            for invalid in (None, True, "", "x" * 129, "bad\x00id"):
                expected["placement"][key] = invalid
                with self.subTest(key=key, invalid=invalid), self.assertRaises(ValueError):
                    target.validate_target(expected)
            expected["placement"][key] = original
        expected["placement"]["unrecognized_id"] = "must-not-pass"
        with self.assertRaises(ValueError):
            target.validate_target(expected)

    def test_startup_and_refresh_require_explicit_authoritative_target_agreement(self):
        strong = execution_target(os.path.realpath(self.root), "desktop")
        invalid = [{"type": "binding_status"}, {"type": "error"},
                   {"type": "binding_status", "native_target": strong}]
        for status in invalid:
            with self.subTest(status=status), patch.object(device.DeviceRouter,
                    "verify_target_registry", VERIFY_TARGET_REGISTRY), patch.object(
                    device.DeviceRouter, "read_wake_status", return_value=status):
                with self.assertRaisesRegex(ValueError, "core binding authority"):
                    device.DeviceRouter(self.args())
        router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        original = dict(router.all_bindings)
        router.verify_target_registry = VERIFY_TARGET_REGISTRY.__get__(router)
        self.profile["bindings"][0]["epoch"] = 2
        self.save()
        with patch.object(router, "read_wake_status", return_value={"type": "binding_status"}):
            with self.assertRaises(ValueError):
                router.refresh_profile()
        self.assertEqual(router.all_bindings, original)

    def revoked_status(self, revoked):
        agreed = {"type": "binding_status", "native_target": None,
                  "last_receipted_sequence": 0, "first_unread_sequence": None}
        # Accepts a bound or unbound call: (router, grant, timeout) or (grant, timeout).
        return lambda *call: ({"type": "error", "code": "invalid_binding", "message": "revoked"}
                              if call[-2]["session"] in revoked else dict(agreed))

    def test_startup_quarantines_a_revoked_binding_instead_of_aborting_the_device(self):
        with patch.object(device.DeviceRouter, "verify_target_registry", VERIFY_TARGET_REGISTRY), \
                patch.object(device.DeviceRouter, "read_wake_status", self.revoked_status({SESSION})):
            router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        self.assertEqual(router.quarantined, {SESSION})
        self.assertEqual(router.feed.keys, {("project", "claude")})
        self.assertEqual(set(router.bindings), {CLAUDE})
        self.assertIsNone(router.native_binding(SESSION))

    def test_refresh_quarantines_a_revoked_binding_instead_of_rejecting_the_profile(self):
        router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        router.verify_target_registry = VERIFY_TARGET_REGISTRY.__get__(router)
        with patch.object(router, "read_wake_status", self.revoked_status({CLAUDE})):
            router.refresh_profile()
        self.assertEqual(router.quarantined, {CLAUDE})
        self.assertEqual(router.feed.keys, {("project", "codex")})
        self.assertEqual(set(router.bindings), set())

    def test_target_disagreement_still_rejects_startup_beside_a_revoked_binding(self):
        mismatch = {"type": "binding_status", "native_target": execution_target(self.root, "desktop")}
        def status(_router, grant, _timeout):
            if grant["session"] == SESSION:
                return {"type": "error", "code": "invalid_binding"}
            return mismatch
        with patch.object(device.DeviceRouter, "verify_target_registry", VERIFY_TARGET_REGISTRY), \
                patch.object(device.DeviceRouter, "read_wake_status", status), \
                self.assertRaisesRegex(ValueError, "core binding authority"):
            device.DeviceRouter(self.args())

    def test_profile_edit_and_v1_downgrade_cannot_weaken_persisted_epoch_target(self):
        persisted = execution_target(os.path.realpath(self.root), "desktop")
        edited = json.loads(json.dumps(persisted))
        edited["profile"] = "unknown"
        status = {"type": "binding_status", "native_target": persisted}
        self.assertFalse(device.claude.native_target.target_agrees(edited, status))
        self.assertFalse(device.claude.native_target.target_agrees(None, status))
        self.assertTrue(device.claude.native_target.target_agrees(persisted, status))
        self.assertTrue(device.claude.native_target.target_agrees(None,
            {"type": "binding_status", "native_target": None}))

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

    def test_codex_recovery_re_queues_the_oldest_unread_and_reports_it(self):
        recoveries, reports = [], []
        class FakeQueue:
            def __init__(self, executable, session, event=None):
                self.session = session
            def __call__(self, hint):
                return True
            def recover(self, hint):
                recoveries.append(hint)
                self.reporter(hint["sequence"], "accepted", None, True, 1)
                return True
        with patch.object(device.codex, "QueueDelivery", FakeQueue):
            router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        router.wake_sender = lambda grant, request: reports.append(
            (grant["member"], request["through"], request["kind"], request["recovery"])) or {
            "type": "woken", "covered": 1}
        router.read_wake_status = lambda grant, _timeout: {
            "type": "binding_status", "first_unread_sequence": 3,
            "recovery_due": grant["member"] == "codex"}
        self.assertEqual(router.recover_once(), 1)
        self.assertEqual(recoveries, [{"context": "project", "member": "codex", "sequence": 3}])
        self.assertTrue(router.flush_wake_reports(2))
        self.assertEqual(reports, [("codex", 3, "codex_queue", True)])

    def test_a_recovery_is_not_repeated_until_the_core_acknowledges_its_report(self):
        # Codex review 3: the core counts recovery attempts from wake reports. A lost report
        # must not let the driver re-wake the same item every loop.
        recoveries, replies = [], []
        class FakeQueue:
            def __init__(self, executable, session, event=None):
                self.session = session
            def __call__(self, hint):
                return True
            def recover(self, hint):
                recoveries.append(hint["sequence"])
                self.reporter(hint["sequence"], "accepted", None, True, 1)
                return True
        with patch.object(device.codex, "QueueDelivery", FakeQueue):
            router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, router)
        status = {"first": 3}
        router.read_wake_status = lambda grant, _timeout: {
            "type": "binding_status", "first_unread_sequence": status["first"],
            "recovery_due": grant["member"] == "codex"}
        router.wake_sender = lambda grant, request: replies.pop(0)
        replies.append({"type": "error", "code": "busy"})
        self.assertEqual(router.recover_once(), 1)
        self.assertTrue(router.flush_wake_reports(2))
        self.assertEqual(router.recover_once(), 0, "the lost report was not acknowledged")
        self.assertEqual(recoveries, [3])
        status["first"] = 4
        replies.append({"type": "woken", "covered": 1})
        self.assertEqual(router.recover_once(), 1, "the member moved on")
        self.assertTrue(router.flush_wake_reports(2))
        replies.append({"type": "woken", "covered": 1})
        self.assertEqual(router.recover_once(), 1, "acknowledged: the core's budget decides")
        self.assertEqual(recoveries, [3, 4, 4])

    def test_claude_is_channel_only_before_subscribe_after_refresh_and_restart(self):
        def assert_no_fallback(router, session):
            self.assertIn(session, router.channel_sessions)
            self.assertNotIn(session, router.channels)
            router.core_call = lambda *_args: self.fail("unconnected channel froze an inbox batch")
            router.wake_status = lambda *_args: {"type": "binding_status", "first_unread_sequence": 7}
            with patch.object(router, "live_socket", side_effect=AssertionError("historical socket lookup")):
                self.assertEqual(router.notify(session), {"type": "skipped"})
                self.assertEqual(router.notify(session, force=True),
                                 {"type": "binding_status", "first_unread_sequence": 7})
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


class NativeHttpTests(DeviceTests):
    """Actual Unix socket framing with explicitly synthetic native process facts.

    These tests verify driver contracts; they are not native Claude uptake evidence.
    """
    def setUp(self):
        super().setUp()
        self.router = device.DeviceRouter(self.args())
        self.addCleanup(self.cleanup_router, self.router)
        self.pid = os.getpid()
        self.sessions = self.root / "sessions"
        self.sessions.mkdir()
        self.start = 1700000000
        self.facts = {"uid": os.getuid(), "ppid": self.pid + 123, "start": self.start}
        self.record = {"pid": self.pid, "sessionId": CLAUDE,
                       "procStart": time.strftime("%a %b %d %H:%M:%S %Y", time.gmtime(self.start))}
        self.save_record()
        fake = patch.object(device.claude, "procinfo", side_effect=lambda pid: dict(self.facts)
                            if pid == self.pid else None)
        fake.start()
        self.addCleanup(fake.stop)
        duration = patch.object(device.native, "WAIT_SECONDS", 0.15)
        duration.start()
        self.addCleanup(duration.stop)
        self.state = {"type": "binding_status", "first_unread_sequence": None}
        self.calls = []
        def status(grant, timeout):
            self.calls.append((dict(grant), timeout))
            return dict(self.state)
        self.router.wake_status = status
        self.router.core_call = lambda *_args: self.fail("wake called consuming/MCP core path")
        self.reports = []
        self.router.wake_sender = lambda grant, request: self.reports.append(
            (grant["member"], request)) or {"type": "woken", "covered": 1}

    def save_record(self):
        (self.sessions / f"{self.pid}.json").write_text(json.dumps(self.record))

    def begin(self, request=b"GET /cbc/v1/wake?after= HTTP/1.1\r\nHost: localhost\r\n\r\n"):
        # Real connected AF_UNIX pair, real peer_credentials, real serve framing.
        client, server = socket.socketpair(socket.AF_UNIX)
        client.settimeout(2)
        self.assertTrue(self.router.slots.acquire(blocking=False))
        worker = threading.Thread(target=self.router.serve, args=(server,))
        worker.start()
        self.addCleanup(worker.join, 2)
        self.addCleanup(client.close)
        try:
            client.sendall(request)
        except BrokenPipeError:
            # An over-limit request may be rejected before the kernel finishes
            # accepting its bytes. The already-written HTTP error still matters.
            pass
        return client, worker

    def finish(self, client, worker):
        raw = b""
        while b"\r\n\r\n" not in raw:
            part = client.recv(4096)
            self.assertTrue(part, "HTTP response headers incomplete")
            raw += part
        headers, payload = raw.split(b"\r\n\r\n", 1)
        lines = headers.split(b"\r\n")
        values = dict(line.split(b": ", 1) for line in lines[1:])
        length = int(values[b"Content-Length"])
        while len(payload) < length:
            part = client.recv(min(4096, length - len(payload)))
            self.assertTrue(part, "HTTP response body incomplete")
            payload += part
        self.assertEqual(length, len(payload))
        self.assertEqual(values[b"Connection"], b"close")
        # Linux may reset a Unix connection when the server closes with rejected
        # oversized request bytes unread. Content-Length defines completion; a
        # further EOF probe must not discard a complete, valid error response.
        worker.join(2)
        self.assertFalse(worker.is_alive())
        return int(lines[0].split()[1]), json.loads(payload) if payload else None

    def exchange(self, request=None):
        return self.finish(*self.begin() if request is None else self.begin(request))

    def test_complete_response_succeeds_before_reset_but_truncation_still_fails(self):
        class ResetClient:
            """Synthetic response stream with Linux-style reset after its bytes."""
            def __init__(self, chunks):
                self.chunks = iter(chunks)
                self.reads = 0

            def recv(self, _limit):
                self.reads += 1
                try:
                    return next(self.chunks)
                except StopIteration:
                    raise ConnectionResetError(104, "connection reset")

        worker = SimpleNamespace(join=lambda _timeout: None, is_alive=lambda: False)
        response = device.native.response(400)
        client = ResetClient([response[:15], response[15:]])
        self.assertEqual(self.finish(client, worker), (400, None))
        self.assertEqual(client.reads, 2)
        # A partial declared body must still reach that reset and fail verification.
        partial = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{"
        with self.assertRaises(ConnectionResetError):
            self.finish(ResetClient([partial]), worker)

    def wait_parked(self):
        deadline = time.monotonic() + 1
        while time.monotonic() < deadline:
            if self.router.native_pending and self.calls:
                return
            time.sleep(0.001)
        self.fail("request did not park")

    def test_uds_empty_heartbeat_has_no_content_or_idle_core_poll(self):
        feed = self.router.feed
        events = []
        with patch.object(self.router, "event", side_effect=lambda kind, **fields:
                          events.append((kind, fields))):
            self.assertEqual(self.exchange(), (204, None))
            self.assertEqual(self.exchange(), (204, None))
        self.assertEqual(events, [("native_mod_attached", {"member": "claude"})])
        self.assertEqual(len(self.calls), 2)
        self.assertIs(self.router.feed, feed)
        self.assertFalse(feed.started)
        self.assertEqual(self.router.native_pending, set())

    def test_unread_only_exact_payload_stable_cursor_and_downward_change(self):
        self.state["first_unread_sequence"] = 99
        events = []
        capture = patch.object(self.router, "event", side_effect=lambda kind, **fields:
                               events.append((kind, fields)))
        capture.start()
        self.addCleanup(capture.stop)
        status, body = self.exchange()
        self.assertEqual(status, 200)
        self.assertEqual(set(body), {"v", "type", "cursor"})
        self.assertEqual(body["v"], 1)
        self.assertEqual(body["type"], "pending")
        token = body["cursor"]
        self.assertRegex(token, "^[0-9a-f]{64}$")
        self.assertEqual(self.exchange()[1]["cursor"], token)
        request = f"GET /cbc/v1/wake?after={token} HTTP/1.1\r\n\r\n".encode()
        self.assertEqual(self.exchange(request), (204, None))
        self.state["first_unread_sequence"] = 1
        status, body = self.exchange(request)
        self.assertEqual(status, 200)
        self.assertNotEqual(body["cursor"], token)
        grant = self.profile["bindings"][1]
        self.assertNotEqual(device.native.cursor(grant, 1), device.native.cursor(grant | {"epoch": 2}, 1))
        for key in ("context", "member", "device", "session"):
            self.assertNotEqual(device.native.cursor(grant, 1),
                                device.native.cursor(grant | {key: "replacement"}, 1))
        self.state["first_unread_sequence"] = None
        self.assertEqual(self.exchange(request), (204, None))
        self.assertEqual(events, [("native_mod_attached", {"member": "claude"}),
            ("native_mod_pending", {"member": "claude"}),
            ("native_mod_pending", {"member": "claude"}),
            ("native_mod_pending", {"member": "claude"})])

    def test_shared_feed_wakes_request_without_check_or_another_feed(self):
        with patch.object(device.native, "WAIT_SECONDS", 2):
            client, worker = self.begin()
            self.wait_parked()
            feed = self.router.feed
            self.state["first_unread_sequence"] = 7
            feed.dispatch({"type": "members_changed", "sequence": 7, "members": [
                {"context": "project", "member": "claude", "sequence": 7}]}, 0)
            self.assertEqual(self.finish(client, worker)[0], 200)
            self.assertIs(self.router.feed, feed)
            self.assertTrue(feed.deliver_ready())
            self.assertEqual(feed.pending_count(), 0)
            self.assertLessEqual(len(self.calls), 4)

    def test_duplicate_wait_returns_429_and_reconnect_releases_session(self):
        client, worker = self.begin()
        self.wait_parked()
        self.assertEqual(self.exchange(), (429, None))
        self.assertEqual(self.finish(client, worker), (204, None))
        self.state["first_unread_sequence"] = 2
        self.assertEqual(self.exchange()[0], 200)

    def test_native_delivery_mode_survives_wait_gap_without_consuming_check(self):
        self.assertEqual(self.exchange(), (204, None))
        self.assertEqual(self.router.native_pending, set())
        self.assertIn(CLAUDE, self.router.native_sessions)
        self.assertTrue(self.router.deliver_hint(CLAUDE))
        self.assertEqual(len(self.calls), 1)

    def mcp_request(self, request):
        with patch.object(device.claude, "corroborate", return_value=CLAUDE):
            return self.router.handle_request(None, self.pid, os.getuid(),
                                              raw=json.dumps(request).encode() + b"\n")

    def test_native_mcp_hello_and_receipt_wake_without_check_or_dead_chat_log(self):
        self.router.native_sessions.add(CLAUDE)
        self.state["first_unread_sequence"] = 7
        events = []
        calls = []
        self.router.core_call = lambda grant, request: calls.append(request) or {"type": "receipted"}
        with patch.object(self.router, "event", side_effect=lambda kind, **fields:
                          events.append((kind, fields))):
            generation = self.router.native_generation
            self.assertEqual(self.mcp_request({"v": 1, "op": "hello"}), {"ok": True})
            self.assertGreater(self.router.native_generation, generation)
            generation = self.router.native_generation
            self.assertEqual(self.mcp_request({"v": 1, "op": "receipt", "reference": "fixture"}),
                             {"ok": True})
            self.assertGreater(self.router.native_generation, generation)
        self.assertEqual(calls, [{"op": "receipt", "reference": "fixture"}])
        self.assertEqual(len(self.calls), 2)
        self.assertEqual([kind for kind, _fields in events], ["hello", "receipt"])

    def test_native_hello_and_receipt_errors_and_auth_are_preserved(self):
        self.router.native_sessions.add(CLAUDE)
        self.state = {"type": "error", "code": "invalid_binding", "message": "revoked"}
        self.assertEqual(self.mcp_request({"v": 1, "op": "hello"}), {"error": "revoked"})
        self.router.core_call = lambda *_args: {"type": "error", "message": "receipt rejected"}
        self.assertEqual(self.mcp_request({"v": 1, "op": "receipt", "reference": "fixture"}),
                         {"error": "receipt rejected"})
        self.state = {"type": "error", "message": "offline"}
        self.assertEqual(self.mcp_request({"v": 1, "op": "hello"}), {"error": "offline"})
        self.assertEqual(self.mcp_request({"v": 1, "op": "hello", "session": CLAUDE}),
                         {"error": "invalid request"})
        count = len(self.calls)
        with patch.object(device.claude, "corroborate", return_value=None):
            self.assertIn("error", self.router.handle_request(None, self.pid, os.getuid(),
                          raw=b'{"v":1,"op":"hello"}\n'))
        self.assertEqual(len(self.calls), count)

    def test_unconnected_channel_hello_is_non_consuming_and_connected_channel_is_unchanged(self):
        self.state["first_unread_sequence"] = 7
        self.assertEqual(self.mcp_request({"v": 1, "op": "hello"}), {"ok": True})
        self.assertEqual(len(self.calls), 1)
        self.assertNotIn(CLAUDE, self.router.native_sessions)
        client, channel = socket.socketpair(socket.AF_UNIX)
        self.addCleanup(client.close)
        self.addCleanup(channel.close)
        self.router.channels[CLAUDE] = (channel, threading.Lock())
        checks = []
        self.router.core_call = lambda grant, request: checks.append(request) or {
            "type": "batch", "reference": "fixture", "items": []}
        self.assertEqual(self.mcp_request({"v": 1, "op": "hello"}), {"ok": True})
        self.assertEqual(checks, [{"op": "check"}])
        self.assertEqual(client.recv(4096), b'{"type":"changed"}\n')
        self.assertEqual(len(self.calls), 1)

    def test_v2_http_wake_checks_actual_cwd_record_and_literal_entrypoint(self):
        actual = os.path.realpath(self.root)
        expected = execution_target(actual, "cli")
        self.router.native_targets[CLAUDE] = {"host": "claude", "target": expected}
        self.record.update(cwd=actual, entrypoint="cli")
        self.save_record()
        self.state["first_unread_sequence"] = 7
        with patch.object(device.claude.native_target, "process_cwd", return_value=actual):
            self.assertEqual(self.exchange()[0], 200)
            for field, value in (("profile", "desktop"), ("cwd", actual + "/different"),
                                 ("saved_environment", {"status": "configured", "id": "not-exposed"})):
                original = expected[field]
                expected[field] = value
                self.calls.clear()
                with self.subTest(field=field):
                    self.assertEqual(self.exchange(), (401, None))
                    self.assertEqual(self.calls, [])
                expected[field] = original
            self.record["cwd"] = actual + "/false-host-record"
            self.save_record()
            self.assertEqual(self.exchange(), (401, None))
            self.record["cwd"] = actual
            self.record["entrypoint"] = "desktop"
            self.save_record()
            expected["profile"] = "desktop"
            self.assertEqual(self.exchange(), (401, None))

    def test_v2_mcp_target_mismatch_blocks_every_operation_before_core(self):
        actual = os.path.realpath(self.root)
        expected = execution_target(actual, "desktop")
        self.router.native_targets[CLAUDE] = {"host": "claude", "target": expected}
        observed = {"session": CLAUDE, "cwd": actual, "profile": "cli",
                    "saved_project": None, "saved_environment": None}
        self.router.core_call = lambda *_args: self.fail("mismatched target reached core")
        with patch.object(device.claude.native_target, "claude_observation", return_value=observed):
            for op in ("hello", "subscribe", "check", "send", "receipt", "discover", "connect"):
                with self.subTest(op=op):
                    self.assertEqual(self.mcp_request({"v": 1, "op": op}),
                                     {"error": "native execution target mismatch"})
            expected["profile"] = "cli"
            requests = []
            self.router.core_call = lambda _binding, request: requests.append(request) or {"type": "empty"}
            self.assertEqual(self.mcp_request({"v": 1, "op": "check"}), {"items": []})
            self.assertEqual(requests, [{"op": "check"}])

    def test_v2_native_observation_refuses_adapter_cwd_and_pid_reuse(self):
        actual = os.path.realpath(self.root)
        self.record.update(cwd=actual, entrypoint="cli")
        self.save_record()
        target = device.claude.native_target
        child_pid = self.pid + 1
        def facts(pid):
            if pid == self.pid:
                return dict(self.facts)
            if pid == child_pid:
                return {"uid": os.getuid(), "start": self.start, "ppid": self.pid}
        def cwd(pid):
            return actual if pid == self.pid else actual + "/wrong-adapter"
        self.assertIsNone(target.claude_observation(self.pid, os.getuid(), self.sessions,
            facts, device.claude.record_start, adapter_pid=child_pid, cwd_provider=cwd))
        calls = 0
        def replaced(pid):
            nonlocal calls
            calls += 1
            return dict(self.facts, start=self.start + (calls > 1))
        self.assertIsNone(target.claude_observation(self.pid, os.getuid(), self.sessions,
            replaced, device.claude.record_start, cwd_provider=lambda _pid: actual))

    def test_http_wake_refuses_missing_or_stronger_authoritative_target(self):
        self.router.wake_status = device.DeviceRouter.wake_status.__get__(self.router)
        self.router.read_wake_status = lambda *_args: dict(self.state)
        self.state["first_unread_sequence"] = 7
        # Legacy profile cannot treat an absent field as legacy authority.
        self.assertEqual(self.exchange(), (401, None))
        self.state["native_target"] = execution_target(os.path.realpath(self.root), "desktop")
        self.assertEqual(self.exchange(), (401, None))
        self.state["native_target"] = None
        self.assertEqual(self.exchange()[0], 200)
        self.state = {"type": "error"}
        self.assertEqual(self.exchange(), (503, None))

    def test_revocation_during_wait_and_shutdown_cancel_promptly(self):
        for stop in (False, True):
            with self.subTest(shutdown=stop), patch.object(device.native, "WAIT_SECONDS", 2):
                self.calls.clear()
                client, worker = self.begin()
                self.wait_parked()
                if stop:
                    self.router.feed.stop()
                    self.router.wake_native()
                else:
                    self.state = {"type": "error", "code": "invalid_binding"}
                    self.router.wake_native()
                self.assertEqual(self.finish(client, worker), (503 if stop else 401, None))
                self.state = {"type": "binding_status", "first_unread_sequence": None}

    def test_profile_quarantine_and_identity_loss_during_wait(self):
        client, worker = self.begin()
        self.wait_parked()
        with self.router.registry_lock:
            self.router.quarantined.add(CLAUDE)
            self.router.install_registry()
        self.assertEqual(self.finish(client, worker), (401, None))
        self.router.quarantined.clear()
        self.router.install_registry()
        self.calls.clear()
        client, worker = self.begin()
        self.wait_parked()
        self.facts["start"] += 1
        self.router.wake_native()
        self.assertEqual(self.finish(client, worker), (401, None))

    def test_direct_pid_not_adapter_parent_and_session_correspondence(self):
        self.assertEqual(self.router.native_identity(self.pid, os.getuid()), CLAUDE)
        self.assertIsNone(self.router.native_identity(self.pid + 123, os.getuid()))
        self.assertIsNone(self.router.native_identity(self.pid, os.getuid() + 1))
        for changes in ({"pid": self.pid + 1}, {"sessionId": SESSION}, {"procStart": "wrong"}):
            original = dict(self.record)
            self.record.update(changes)
            self.save_record()
            self.assertEqual(self.exchange(), (401, None))
            self.record = original
        self.assertEqual(self.calls, [])

    def test_status_failure_is_503_and_revocation_only_is_401(self):
        for state in ({"type": "error"}, {"type": "batch"}, {"type": "binding_status"},
                      {"type": "binding_status", "first_unread_sequence": True},
                      {"type": "binding_status", "first_unread_sequence": -1}):
            self.state = state
            self.assertEqual(self.exchange(), (503, None))
        self.state = {"type": "error", "code": "invalid_binding"}
        self.assertEqual(self.exchange(), (401, None))

    def test_revalidate_core_and_identity_before_pending_response(self):
        self.state["first_unread_sequence"] = 1
        states = iter([dict(self.state), {"type": "error", "code": "invalid_binding"}])
        self.router.wake_status = lambda *_args: next(states)
        events = []
        with patch.object(self.router, "event", side_effect=lambda kind, **fields:
                          events.append((kind, fields))):
            self.assertEqual(self.exchange(), (401, None))
        self.assertEqual(events, [("native_mod_attached", {"member": "claude"})])

    def test_wake_status_actual_core_uds_is_only_non_consuming_status(self):
        observed = []
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(self.router.core)
        listener.listen(2)
        listener.settimeout(2)
        self.addCleanup(listener.close)
        def core():
            for _ in range(2):
                conn, _ = listener.accept()
                with conn:
                    observed.append(json.loads(device.claude.read_frame(conn)))
                    conn.sendall(b'{"type":"binding_status","first_unread_sequence":7,"native_target":null}\n')
        server = threading.Thread(target=core)
        server.start()
        self.addCleanup(server.join, 2)
        self.router.wake_status = device.DeviceRouter.wake_status.__get__(self.router)
        self.assertEqual(self.exchange()[0], 200)
        server.join(2)
        self.assertEqual(len(observed), 2)
        for request in observed:
            self.assertEqual(request["auth"], self.profile["bindings"][1])
            self.assertEqual(request["request"], {"op": "binding_status"})

    def test_header_timeout_is_bounded(self):
        with patch.object(device.native, "HEADER_SECONDS", 0.05):
            self.assertEqual(self.exchange(b"GET /cbc/v1/wake?after= HTTP/1.1\r\n"), (400, None))
        self.assertEqual(self.calls, [])

    def test_malformed_query_headers_methods_and_bounds_never_reach_core(self):
        invalid = [b"POST /cbc/v1/wake?after= HTTP/1.1\r\n\r\n",
            b"GET /cbc/v1/wake HTTP/1.1\r\n\r\n",
            b"GET /cbc/v1/wake?after=&session=chosen HTTP/1.1\r\n\r\n",
            b"GET /cbc/v1/wake?after=" + b"A" * 64 + b" HTTP/1.1\r\n\r\n",
            b"GET /cbc/v1/wake?after= HTTP/1.1\r\nContent-Length: 1\r\n\r\nx",
            b"GET /cbc/v1/wake?after= HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
            b"GET /cbc/v1/wake?after= HTTP/1.1\r\nX: a\r\nX: b\r\n\r\n",
            b"GET /cbc/v1/wake?after= HTTP/1.1\r\n folded\r\n\r\n",
            b"GET /cbc/v1/wake?after= HTTP/1.1\r\nX: " + b"x" * 9000 + b"\r\n\r\n"]
        for request in invalid:
            with self.subTest(request=request[:80]):
                self.assertEqual(self.exchange(request), (400, None))
        self.assertEqual(self.calls, [])

    def test_waiter_budget_is_bounded_without_allocating_another_feed(self):
        for _ in range(device.native.MAX_WAITERS):
            self.assertTrue(self.router.native_slots.acquire(blocking=False))
        self.assertEqual(self.exchange(), (429, None))
        for _ in range(device.native.MAX_WAITERS):
            self.router.native_slots.release()
        self.assertEqual(self.calls, [])

    def test_raw_json_mcp_and_codex_nudge_remain_newline_json(self):
        self.router.core_call = lambda *_args: {"type": "binding_status", "first_unread_sequence": None}
        client, worker = self.begin((json.dumps({"v": 1, "op": "codex_receipt",
            "auth": self.profile["bindings"][0]}) + "\n").encode())
        raw = client.recv(4096)
        worker.join(2)
        self.assertEqual(json.loads(raw), {"ok": True})
        with patch.object(device.claude, "corroborate", return_value=CLAUDE):
            self.router.core_call = lambda *_args: {"type": "empty"}
            client, worker = self.begin(b'{"v":1,"op":"check"}\n')
            raw = client.recv(4096)
            worker.join(2)
            self.assertEqual(json.loads(raw), {"items": []})

    # #113 B1: a wake is reported only after an actual write, and recovery bypasses every latch.
    def wake_reports(self):
        self.assertTrue(self.router.flush_wake_reports(2))
        reports = []
        for member, request in self.reports:
            self.assertEqual(request["op"], "wake")
            self.assertIs(type(request["attempt_wall_us"]), int)
            reports.append((member, {k: v for k, v in request.items()
                                     if k not in ("op", "attempt_wall_us")}))
        self.reports.clear()
        return reports

    def connect_channel(self):
        client, channel = socket.socketpair(socket.AF_UNIX)
        self.addCleanup(client.close)
        self.addCleanup(channel.close)
        self.router.channels[CLAUDE] = (channel, threading.Lock())
        return client, channel

    # Codex review 2: the attempt time is taken before the host is notified, so a receipt the
    # notification causes is always later than it. The fakes stamp the write and then stall.
    def test_a_channel_wake_reports_the_time_before_the_bell_was_written(self):
        written = []
        def ring(_session, _path=None):
            written.append(time.time_ns() // 1000)
            time.sleep(0.02)
            return True
        self.connect_channel()
        self.router.ring = ring
        self.router.core_call = lambda *_args: {"type": "batch", "reference": "r1", "items": [
            {"sequence": 5, "room": "r", "sender": "a", "kind": "message", "body": "x"}]}
        self.assertTrue(self.router.notify(CLAUDE)["_notified"])
        self.assertTrue(self.router.flush_wake_reports(2))
        (_member, request), = self.reports
        self.assertLessEqual(request["attempt_wall_us"], written[0])

    def test_a_codex_wake_reports_the_time_before_the_queue_command_ran(self):
        ran = []
        def runner(*_args, **_kwargs):
            ran.append(time.time_ns() // 1000)
            time.sleep(0.02)
            return SimpleNamespace(returncode=0)
        binding = next(b for b in self.profile["bindings"] if b["member"] == "codex")
        callback = device.codex.QueueDelivery("/usr/local/bin/codex", SESSION, runner=runner,
            reporter=self.router.codex_reporter(binding))
        self.assertTrue(callback(HINT))
        self.assertTrue(self.router.flush_wake_reports(2))
        (_member, request), = self.reports
        self.assertLessEqual(request["attempt_wall_us"], ran[0])

    def test_a_channel_write_reports_one_wake_through_the_batch(self):
        client, channel = self.connect_channel()
        batch = {"type": "batch", "reference": "r1", "items": [
            {"sequence": 5, "room": "r", "sender": "a", "kind": "message", "body": "x"},
            {"sequence": 9, "room": "r", "sender": "a", "kind": "message", "body": "y"}]}
        self.router.core_call = lambda *_args: dict(batch)
        self.assertTrue(self.router.notify(CLAUDE)["_notified"])
        self.assertEqual(client.recv(4096), b'{"type":"changed"}\n')
        self.assertEqual(self.wake_reports(), [("claude", {"through": 9, "kind": "claude_channel",
            "outcome": "accepted", "recovery": False})])
        self.assertTrue(self.router.notify(CLAUDE)["_notified"])
        self.assertEqual(self.wake_reports(), [], "a batch already rung is not a new wake")
        channel.close()
        self.router.core_call = lambda *_args: dict(batch, reference="r2")
        self.assertFalse(self.router.notify(CLAUDE)["_notified"])
        self.assertEqual(self.wake_reports(), [("claude", {"through": 9, "kind": "claude_channel",
            "outcome": "failed", "failure": "channel_write_failed", "recovery": False})])

    def test_a_lost_report_never_changes_delivery(self):
        client, _channel = self.connect_channel()
        self.router.core_call = lambda *_args: {"type": "batch", "reference": "r1", "items": [
            {"sequence": 3, "room": "r", "sender": "a", "kind": "message", "body": "x"}]}
        events = []
        capture = patch.object(self.router, "event", side_effect=lambda kind, **fields:
                               events.append((kind, fields)))
        capture.start()
        self.addCleanup(capture.stop)
        for sender in (lambda *_args: {"type": "error", "message": "core unavailable"},
                       lambda *_args: (_ for _ in ()).throw(OSError("down"))):
            self.router.wake_sender = sender
            self.router.rung.clear()
            self.assertTrue(self.router.notify(CLAUDE)["_notified"])
            self.assertEqual(client.recv(4096), b'{"type":"changed"}\n')
            self.assertTrue(self.router.flush_wake_reports(2))
        self.assertEqual([kind for kind, _ in events].count("wake_report_failed"), 2)
        self.assertNotIn("down", repr(events))

    def test_a_mod_pending_reports_a_wake_through_the_latest_hint(self):
        self.router.feed.dispatch({"type": "members_changed", "sequence": 12, "members": [
            {"context": "project", "member": "claude", "sequence": 12}]}, 0)
        self.assertTrue(self.router.feed.deliver_ready())
        self.state["first_unread_sequence"] = 4
        status, body = self.exchange()
        self.assertEqual(status, 200)
        self.assertEqual(self.wake_reports(), [("claude", {"through": 12, "kind": "claude_mod",
            "outcome": "accepted", "recovery": False})])
        request = f"GET /cbc/v1/wake?after={body['cursor']} HTTP/1.1\r\n\r\n".encode()
        self.assertEqual(self.exchange(request), (204, None))
        self.assertEqual(self.wake_reports(), [], "an unchanged cursor is not a new wake")

    def test_a_mod_woken_before_the_hint_is_delivered_still_reports_the_hint(self):
        # feed_changed wakes a parked waiter before the delivery worker runs the handler.
        self.router.feed.dispatch({"type": "members_changed", "sequence": 12, "members": [
            {"context": "project", "member": "claude", "sequence": 12}]}, 0)
        self.state["first_unread_sequence"] = 4
        status, _body = self.exchange()
        self.assertEqual(status, 200)
        self.assertEqual(self.wake_reports(), [("claude", {"through": 12, "kind": "claude_mod",
            "outcome": "accepted", "recovery": False})])

    def test_recovery_re_hints_a_parked_mod_once_even_with_an_unchanged_cursor(self):
        self.state["first_unread_sequence"] = 4
        status, body = self.exchange()
        self.assertEqual(status, 200)
        self.wake_reports()
        request = f"GET /cbc/v1/wake?after={body['cursor']} HTTP/1.1\r\n\r\n".encode()
        # Only the Claude member is due: the fixture's Codex target must never run a command.
        self.router.read_wake_status = lambda grant, _timeout: dict(
            self.state, recovery_due=grant["member"] == "claude")
        self.assertEqual(self.router.recover_once(), 1)
        self.assertEqual(self.exchange(request), (200, body))
        self.assertEqual(self.wake_reports(), [("claude", {"through": 4, "kind": "claude_mod",
            "outcome": "accepted", "recovery": True})])
        self.assertEqual(self.exchange(request), (204, None), "one recovery, one re-hint")

    def test_recovery_force_rings_a_channel_and_skips_what_is_not_due(self):
        client, _channel = self.connect_channel()
        self.router.core_call = lambda *_args: {"type": "batch", "reference": "r1", "items": [
            {"sequence": 6, "room": "r", "sender": "a", "kind": "message", "body": "x"}]}
        self.assertTrue(self.router.notify(CLAUDE)["_notified"])
        client.recv(4096)
        self.wake_reports()
        statuses = {"claude": {"type": "binding_status", "first_unread_sequence": 6}}
        self.router.read_wake_status = lambda grant, _timeout: dict(
            statuses.get(grant["member"], {"type": "error"}))
        self.assertEqual(self.router.recover_once(), 0, "an old core sends no flag: nothing is due")
        statuses["claude"]["recovery_due"] = False
        self.assertEqual(self.router.recover_once(), 0)
        statuses["claude"]["recovery_due"] = True
        self.assertEqual(self.router.recover_once(), 1)
        self.assertEqual(client.recv(4096), b'{"type":"changed"}\n', "rung although already rung")
        self.assertEqual(self.wake_reports(), [("claude", {"through": 6, "kind": "claude_channel",
            "outcome": "accepted", "recovery": True})])

    def test_recovery_loop_runs_until_the_feed_stops(self):
        passes = []
        self.router.recover_once = lambda: passes.append(1) or 0
        self.router.recovery_interval = 0.01
        loop = threading.Thread(target=self.router.recovery_loop)
        loop.start()
        deadline = time.monotonic() + 2
        while len(passes) < 2 and time.monotonic() < deadline:
            time.sleep(0.01)
        self.router.feed.stop()
        loop.join(2)
        self.assertFalse(loop.is_alive())
        self.assertGreaterEqual(len(passes), 2)


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

    def test_a_queue_command_is_reported_only_when_it_runs(self):
        calls, reports = [], []
        state = {"type": "binding_status", "last_receipted_sequence": 0, "first_unread_sequence": 1}
        outcome = {"value": 0}
        ran = []
        def runner(*args, **_kwargs):
            calls.append(args)
            ran.append(time.time_ns() // 1000)
            if isinstance(outcome["value"], Exception):
                raise outcome["value"]
            return SimpleNamespace(returncode=outcome["value"])
        def reporter(*args):
            # The fifth argument is the attempt time, taken just before this run.
            self.assertIs(type(args[4]), int)
            self.assertLessEqual(args[4], ran[-1])
            reports.append(args[:4])
        callback = device.codex.QueueDelivery("/usr/local/bin/codex", SESSION, runner=runner,
            status_provider=lambda: dict(state), reporter=reporter)
        self.assertTrue(callback(HINT))
        self.assertTrue(callback(HINT | {"sequence": 8}))
        self.assertEqual(len(calls), 1)
        self.assertEqual(reports, [(7, "accepted", None, False)], "a latched hint is not a wake")
        self.assertTrue(callback.recover(HINT | {"sequence": 1}))
        self.assertEqual(len(calls), 2, "recovery bypasses the latch")
        self.assertEqual(reports[-1], (1, "accepted", None, True))
        state.update(last_receipted_sequence=5, first_unread_sequence=6)
        outcome["value"] = 1
        self.assertFalse(callback(HINT))
        self.assertEqual(reports[-1], (7, "failed", "queue_exit", False))
        outcome["value"] = OSError("private path")
        self.assertFalse(callback(HINT))
        self.assertEqual(reports[-1], (7, "failed", "queue_unavailable", False))
        callback.status_provider = lambda: {"type": "error"}
        before = len(reports)
        self.assertFalse(callback(HINT))
        self.assertEqual(len(reports), before, "no command, no attempt")


if __name__ == "__main__":
    unittest.main()
