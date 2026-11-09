"""Deterministic names and cooperative handoff artifacts, without live host actions."""

import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_hierarchy_skills import ROOT, bootstrap, load


titles = load("titles", ROOT / "bins/cbc/skill/cbc-hierarchy/scripts/session_title.py")
handoff = load("handoff", ROOT / "bins/cbc/skill/cbc-handoff/scripts/handoff.py")


class SessionTitleTests(unittest.TestCase):
    def test_all_default_and_themed_roles(self):
        arguments = dict(project="Aevark", repo="API", domain="Billing", feature="Stripe Billing", task="types-1")
        expected = {
            "cbc-project-lead": ("aevark-project-lead", "aevark-emperor"),
            "cbc-repo-lead": ("api-repo-lead", "api-primarch"),
            "cbc-domain-lead": ("api-domain-lead-billing", "api-retired-domain-lead-billing"),
            "cbc-feature-lead": ("api-feature-lead-stripe-billing", "api-chapter-master-stripe-billing"),
            "cbc-task-agent": ("api-task-agent-stripe-billing-types-1", "api-techmarine-stripe-billing-types-1"),
        }
        for role, pair in expected.items():
            for theme, result in zip(("default", "warhammer"), pair):
                with self.subTest(role=role, theme=theme):
                    self.assertEqual(titles.build_title(role, theme=theme, **arguments), result)

    def test_alias_default_and_explicit_override(self):
        for alias in ("emperor", "cbc-emperor"):
            self.assertEqual(titles.build_title(alias, project="aevark/"), "aevark-emperor")
            self.assertEqual(titles.build_title(alias, project="aevark", theme="default"),
                             "aevark-project-lead")
        self.assertEqual(titles.build_title("cbc-primarch", repo="engine"), "engine-primarch")

    def test_slug_normalization_and_stable_parallel_suffix(self):
        self.assertEqual(titles.build_title("cbc-chapter-master", repo="API", feature="Café invoices", suffix="run 2"),
                         "api-chapter-master-cafe-invoices-run-2")

    def test_long_names_are_bounded_repeatable_and_distinct(self):
        args = dict(repo="a" * 100, feature="b" * 100, task="c" * 100, suffix="d" * 100)
        title = titles.build_title("cbc-techmarine", **args)
        self.assertLessEqual(len(title), 200)
        self.assertEqual(title, titles.build_title("cbc-techmarine", **args))
        self.assertNotEqual(title, titles.build_title("cbc-techmarine", **{**args, "task": "c" * 99 + "x"}))

    def test_missing_assignment_fields_fail_instead_of_inventing_names(self):
        for role, args in (("cbc-emperor", {}), ("cbc-primarch", {}),
                           ("cbc-chapter-master", {"repo": "api"}),
                           ("cbc-techmarine", {"repo": "api", "feature": "billing"})):
            with self.subTest(role=role), self.assertRaises(ValueError):
                titles.build_title(role, **args)

    def test_superseded_title_uses_fixed_exact_utc_time(self):
        expected = "api-chapter-master-billing-superseded-20261021T173456123456Z"
        self.assertEqual(titles.superseded_title("api-chapter-master-billing", "2026-10-21T13:34:56.123456-04:00"), expected)
        self.assertEqual(titles.superseded_title("api-chapter-master-billing", "2026-10-21T17:34:56.123456Z"), expected)
        with self.assertRaisesRegex(ValueError, "timezone"):
            titles.superseded_title("api-primarch", "2026-10-21T17:34:56")

    def test_title_cli_emits_only_computed_name(self):
        result = subprocess.run([sys.executable, "-B", str(ROOT / "bins/cbc/skill/cbc-hierarchy/scripts/session_title.py"),
                                 "--role", "cbc-chapter-master", "--repo", "api", "--feature", "billing"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "api-chapter-master-billing\n")


class HandoffTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cbc-handoff-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.state_dir = bootstrap.bootstrap(self.root, "aevark", "api-billing", "cbc-feature-lead", "warhammer")
        state = self.state_dir / "state.md"
        state.write_text(state.read_text().replace("connections:\n", "connections:\n  parent: room-a --as aevark--api-billing --model test\n"))

    def prepare(self):
        return handoff.prepare(self.state_dir, "old-native-id", "api-chapter-master-billing", {"room-a": 123})

    def test_prepare_is_structured_immutable_snapshot_without_takeover(self):
        directory = self.prepare()
        manifest = handoff.verify(directory)
        self.assertEqual(manifest["last_handled"], {"room-a": 123})
        self.assertEqual(manifest["label"], "aevark--api-billing")
        self.assertEqual(manifest["predecessor_session"], "old-native-id")
        self.assertFalse((directory / "released.json").exists())
        self.assertFalse((directory / "outcome.json").exists())
        original = (directory / "state.md").read_bytes()
        (self.state_dir / "state.md").write_text("live state changed after export\n")
        self.assertEqual((directory / "state.md").read_bytes(), original)
        handoff.verify(directory)

    def test_accept_requires_release_to_matching_successor(self):
        directory = self.prepare()
        with self.assertRaises(ValueError):
            handoff.accept(directory, "new-native-id", "checked")
        handoff.release(directory, "old-native-id", "new-native-id", "receivers stopped; no native children", {"room-a": 123})
        with self.assertRaisesRegex(ValueError, "different"):
            handoff.accept(directory, "wrong-native-id", "checked")
        receipt = handoff.accept(directory, "new-native-id", "catch-up and receives verified")
        self.assertEqual(receipt["session"], "new-native-id")
        self.assertEqual(handoff.verify(directory)["transfer_state"], "accepted-awaiting-activation")
        with self.assertRaises(FileExistsError):
            handoff.accept(directory, "new-native-id", "duplicate")

    def test_wrong_predecessor_or_same_session_cannot_release(self):
        directory = self.prepare()
        for predecessor, successor in (("other", "new"), ("old-native-id", "old-native-id")):
            with self.assertRaises(ValueError):
                handoff.release(directory, predecessor, successor, "checked", {"room-a": 123})
        self.assertFalse((directory / "released.json").exists())

    def test_snapshot_tampering_blocks_release(self):
        directory = self.prepare()
        (directory / "questions.md").write_text("lost Q-3")
        with self.assertRaisesRegex(ValueError, "snapshot changed"):
            handoff.release(directory, "old-native-id", "new", "checked", {"room-a": 123})

    def test_every_declared_room_needs_nonnegative_handled_sequence(self):
        for handled in ({}, {"room-b": 1}, {"room-a": -1}, {"room-a": True}):
            with self.subTest(handled=handled), self.assertRaises(ValueError):
                handoff.prepare(self.state_dir, "old", "title", handled)
        self.assertFalse((self.state_dir / "handoffs").exists())

    def test_release_evidence_is_required_and_cannot_be_overwritten(self):
        directory = self.prepare()
        with self.assertRaises(ValueError):
            handoff.release(directory, "old-native-id", "new", "", {"room-a": 123})
        handoff.release(directory, "old-native-id", "new", "verified", {"room-a": 123})
        with self.assertRaises(ValueError):
            handoff.release(directory, "old-native-id", "other", "changed mind", {"room-a": 123})

    def test_release_captures_final_checkpoints_answers_and_outbound_refs(self):
        directory = self.prepare()
        (self.state_dir / "questions.md").write_text("Q-3 answered in room-a at seq 128\n")
        outbound = [{"room": "room-a", "seq": 129, "status": "sent"}]
        marker = handoff.release(directory, "old-native-id", "new", "verified", {"room-a": 130}, outbound)
        self.assertEqual(marker["last_handled"], {"room-a": 130})
        self.assertEqual(marker["late_outbound"], outbound)
        self.assertIn("Q-3 answered", (directory / "release-questions.md").read_text())
        self.assertNotIn("Q-3 answered", (directory / "questions.md").read_text())
        handoff.verify(directory)

    def test_release_checkpoint_cannot_regress(self):
        directory = self.prepare()
        with self.assertRaisesRegex(ValueError, "backwards"):
            handoff.release(directory, "old-native-id", "new", "verified", {"room-a": 122})
        self.assertFalse((directory / "released.json").exists())

    def test_revocation_wins_before_acceptance(self):
        directory = self.prepare()
        handoff.release(directory, "old-native-id", "new", "verified", {"room-a": 123})
        handoff.revoke(directory, "old-native-id", "direct user message reference")
        self.assertEqual(handoff.verify(directory)["transfer_state"], "revoked")
        with self.assertRaises(FileExistsError):
            handoff.accept(directory, "new", "checked")

    def test_acceptance_wins_before_revocation_and_activation_is_separate(self):
        directory = self.prepare()
        handoff.release(directory, "old-native-id", "new", "verified", {"room-a": 123})
        accepted = handoff.accept(directory, "new", "checked")
        with self.assertRaises(FileExistsError):
            handoff.revoke(directory, "old-native-id", "user ref")
        with self.assertRaisesRegex(ValueError, "activation"):
            handoff.supersede(directory, "old-native-id")
        with self.assertRaises(ValueError):
            handoff.activate(directory, "wrong", "checked")
        handoff.activate(directory, "new", "receives and catch-up checked")
        marker = handoff.supersede(directory, "old-native-id")
        self.assertEqual(marker["superseded_at"], accepted["at"])
        self.assertEqual(handoff.verify(directory)["transfer_state"], "superseded")

    def test_final_snapshot_tampering_blocks_acceptance(self):
        directory = self.prepare()
        handoff.release(directory, "old-native-id", "new", "verified", {"room-a": 123})
        (directory / "release-state.md").write_text("changed after release")
        with self.assertRaisesRegex(ValueError, "release snapshot changed"):
            handoff.accept(directory, "new", "checked")

    def test_accept_and_revoke_race_has_one_outcome(self):
        directory = self.prepare()
        handoff.release(directory, "old-native-id", "new", "verified", {"room-a": 123})
        barrier = threading.Barrier(2)

        def compete(action):
            barrier.wait(timeout=5)
            try:
                if action == "accept":
                    handoff.accept(directory, "new", "checked")
                else:
                    handoff.revoke(directory, "old-native-id", "user ref")
                return True
            except (OSError, ValueError):
                return False

        with ThreadPoolExecutor(max_workers=2) as executor:
            results = list(executor.map(compete, ("accept", "revoke")))
        self.assertEqual(sum(results), 1)
        self.assertIn(handoff.verify(directory)["transfer_state"], ("accepted-awaiting-activation", "revoked"))

    def test_symlinked_source_is_rejected(self):
        questions = self.state_dir / "questions.md"
        questions.unlink()
        questions.symlink_to(self.state_dir / "brief.md")
        with self.assertRaisesRegex(ValueError, "symlinked"):
            self.prepare()

    def test_cli_prepare_and_verify(self):
        script = ROOT / "bins/cbc/skill/cbc-handoff/scripts/handoff.py"
        result = subprocess.run([sys.executable, "-B", str(script), "prepare", "--state-dir", str(self.state_dir),
                                 "--from-session", "old", "--title", "api-chapter-master-billing",
                                 "--handled", '{"room-a":123}'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        checked = subprocess.run([sys.executable, "-B", str(script), "verify", "--handoff", result.stdout.strip()],
                                 capture_output=True, text=True)
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertEqual(json.loads(checked.stdout)["last_handled"], {"room-a": 123})


if __name__ == "__main__":
    unittest.main()
