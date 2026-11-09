"""Scoped installer and bootstrap tests. No daemon, native chat or home writes."""

import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


installer = load("installer", ROOT / "tools/install_hierarchy_skills.py")
bootstrap = load("bootstrap", ROOT / "bins/cbc/skill/cbc-hierarchy/scripts/bootstrap.py")
handoff = load("room_handoff", ROOT / "bins/cbc/skill/cbc-handoff/scripts/handoff.py")


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cbc-hierarchy-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "source"
        for name in installer.NAMES:
            directory = self.source / name
            (directory / "agents").mkdir(parents=True)
            (directory / "SKILL.md").write_text(f"name: {name}\n")
            (directory / "agents/openai.yaml").write_text("interface: {}\n")

    def test_adds_only_package_and_leaves_legacy_unchanged(self):
        targets = [self.root / "claude", self.root / "codex"]
        for target in targets:
            (target / "cbc-worker").mkdir(parents=True)
            (target / "cbc-worker/SKILL.md").write_text("live legacy worker")
        changes = installer.install(targets, self.source)
        self.assertEqual(len(changes), len(installer.NAMES) * len(targets))
        for target in targets:
            self.assertEqual((target / "cbc-worker/SKILL.md").read_text(), "live legacy worker")
            for name in installer.NAMES:
                self.assertEqual(installer.manifest(target / name),
                                 installer.manifest(self.source / name))
        self.assertEqual(installer.install(targets, self.source), [])

    def test_check_makes_no_directory(self):
        target = self.root / "not-created"
        self.assertEqual(len(installer.install([target], self.source, check=True)), len(installer.NAMES))
        self.assertFalse(target.exists())

    def test_conflict_preflights_all_targets_without_partial_install(self):
        first, second = self.root / "first", self.root / "second"
        (second / installer.NAMES[-1]).mkdir(parents=True)
        conflict = second / installer.NAMES[-1] / "SKILL.md"
        conflict.write_text("user-owned new skill")
        with self.assertRaisesRegex(ValueError, "existing skill differs"):
            installer.install([first, second], self.source)
        self.assertFalse(first.exists())
        self.assertEqual(conflict.read_text(), "user-owned new skill")

    def test_rejects_destination_symlink_without_touching_target(self):
        target, other = self.root / "target", self.root / "other"
        target.mkdir()
        other.mkdir()
        (target / installer.NAMES[0]).symlink_to(other, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlink"):
            installer.install([target], self.source)
        self.assertEqual(list(other.iterdir()), [])

    def test_rejects_symlink_in_source(self):
        source_file = self.source / installer.NAMES[0] / "SKILL.md"
        source_file.unlink()
        source_file.symlink_to(self.source / installer.NAMES[1] / "SKILL.md")
        with self.assertRaisesRegex(ValueError, "symlink"):
            installer.install([self.root / "target"], self.source)

    def test_copies_helper_assets_but_not_python_cache(self):
        scripts = self.source / installer.NAMES[0] / "scripts"
        (scripts / "__pycache__").mkdir(parents=True)
        (scripts / "bootstrap.py").write_text("# helper\n")
        (scripts / "__pycache__/bootstrap.pyc").write_bytes(b"cache")
        target = self.root / "target"
        installer.install([target], self.source)
        self.assertTrue((target / installer.NAMES[0] / "scripts/bootstrap.py").exists())
        self.assertFalse((target / installer.NAMES[0] / "scripts/__pycache__").exists())

    def test_duplicate_target_is_not_installed_twice(self):
        target = self.root / "target"
        self.assertEqual(len(installer.install([target, target], self.source)), len(installer.NAMES))

    def test_incomplete_source_cannot_write(self):
        (self.source / installer.NAMES[-1] / "agents/openai.yaml").unlink()
        target = self.root / "target"
        with self.assertRaisesRegex(ValueError, "incomplete skill"):
            installer.install([target], self.source)
        self.assertFalse(target.exists())

    def test_missing_canonical_target_prevents_alias_install(self):
        (self.source / "cbc-project-lead/SKILL.md").unlink()
        target = self.root / "target"
        with self.assertRaisesRegex(ValueError, "incomplete skill"):
            installer.install([target], self.source)
        self.assertFalse(target.exists())

    def test_real_package_cli_check_is_read_only(self):
        target = self.root / "cli-target"
        result = subprocess.run(
            [sys.executable, "-B", str(ROOT / "tools/install_hierarchy_skills.py"),
             "--check", "--target", str(target)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"Would add {len(installer.NAMES)}", result.stdout)
        self.assertFalse(target.exists())

    def test_real_alias_dependencies_survive_install(self):
        target = self.root / "real-package"
        installer.install([target])
        for alias, canonical in installer.ALIASES.items():
            with self.subTest(alias=alias):
                self.assertTrue((target / alias / "SKILL.md").is_file())
                self.assertTrue((target / alias / "agents/openai.yaml").is_file())
                dependency = target / alias / ".." / canonical / "SKILL.md"
                self.assertEqual(dependency.read_bytes(),
                                 (installer.SOURCE / canonical / "SKILL.md").read_bytes())
        self.assertTrue((target / "cbc-hierarchy/scripts/bootstrap.py").is_file())
        self.assertEqual(len(list(target.iterdir())), len(installer.NAMES))

    def test_role_room_reference_links_resolve_after_two_host_install(self):
        targets = [self.root / "claude", self.root / "codex"]
        installer.install(targets)
        expected = (installer.SOURCE / "cbc-hierarchy/references/rooms.md").read_bytes()
        for target in targets:
            for role in bootstrap.ROLES:
                entrypoint = target / role / "SKILL.md"
                links = re.findall(r"`([^`\n]+/references/rooms\.md)`", entrypoint.read_text())
                self.assertTrue(links, f"missing room-reference route in {role}")
                for link in links:
                    self.assertEqual((entrypoint.parent / link).read_bytes(), expected)
            bootstrap_reference = target / "cbc-project-lead/references/project-bootstrap.md"
            links = re.findall(r"`([^`\n]+/references/rooms\.md)`", bootstrap_reference.read_text())
            self.assertTrue(links)
            for link in links:
                self.assertEqual((bootstrap_reference.parent / link).read_bytes(), expected)

    def test_delegation_reference_resolves_for_installed_repo_lead(self):
        targets = [self.root / "claude", self.root / "codex"]
        installer.install(targets)
        expected = (installer.SOURCE / "cbc-hierarchy/references/delegation.md").read_bytes()
        for target in targets:
            entrypoint = target / "cbc-repo-lead/SKILL.md"
            links = re.findall(r"`([^`\n]+/references/delegation\.md)`", entrypoint.read_text())
            self.assertTrue(links, "missing delegation-reference route")
            for link in links:
                self.assertEqual((entrypoint.parent / link).read_bytes(), expected)


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cbc-role-record-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_creates_small_isolated_owned_templates(self):
        directory = bootstrap.bootstrap(self.root, "project-a", "engine-feature-b",
                                        "cbc-feature-lead", "warhammer")
        self.assertEqual(directory, self.root.resolve() / ".cbc/hierarchy/project-a/engine-feature-b")
        identity = json.loads((directory / "identity.json").read_text())
        self.assertEqual(identity["label"], "project-a--engine-feature-b")
        self.assertEqual(identity["theme"], "warhammer")
        self.assertEqual(len(list(directory.iterdir())), 4)
        self.assertLess(len((directory / "state.md").read_text().splitlines()), 80)
        self.assertLess(len((directory / "brief.md").read_text().splitlines()), 20)
        self.assertEqual(list((self.root / ".cbc").glob("*.md")), [])

    def test_reentry_refuses_instead_of_resetting_state(self):
        directory = bootstrap.bootstrap(self.root, "a", "b", "cbc-feature-lead")
        state = directory / "state.md"
        state.write_text("user is resolving Q-1\n")
        with self.assertRaises(FileExistsError):
            bootstrap.bootstrap(self.root, "a", "b", "cbc-feature-lead")
        self.assertEqual(state.read_text(), "user is resolving Q-1\n")

    def test_rejects_traversal_and_ambiguous_ids(self):
        for value in ("../other", "a/b", "a--b", "Upper", "a\nb", "x" * 65):
            with self.subTest(value=value), self.assertRaises(Exception):
                bootstrap.bootstrap(self.root, value, "member", "cbc-feature-lead")
        self.assertFalse((self.root / ".cbc").exists())

    def test_rejects_symlink_workspace_records(self):
        other = self.root / "other"
        other.mkdir()
        (self.root / ".cbc").symlink_to(other, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlink"):
            bootstrap.bootstrap(self.root, "a", "b", "cbc-feature-lead")
        self.assertEqual(list(other.iterdir()), [])

    def test_role_and_theme_are_validated_without_writes(self):
        for role, theme in (("cbc-worker", "default"), ("cbc-feature-lead", "unknown")):
            with self.assertRaisesRegex(ValueError, "role or theme"):
                bootstrap.bootstrap(self.root, "a", "b", role, theme)
        self.assertFalse((self.root / ".cbc").exists())

    def test_cli_creates_then_refuses_reinitialization(self):
        command = [sys.executable, "-B", str(ROOT / "bins/cbc/skill/cbc-hierarchy/scripts/bootstrap.py"),
                   "--root", str(self.root), "--context", "test-context", "--member", "task-1",
                   "--role", "cbc-feature-lead", "--theme", "warhammer"]
        first = subprocess.run(command, capture_output=True, text=True)
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertIn("NOTE: project root is not a Git worktree", first.stderr)
        self.assertTrue((Path(first.stdout.strip()) / "state.md").is_file())
        second = subprocess.run(command, capture_output=True, text=True)
        self.assertNotEqual(second.returncode, 0)
        self.assertIn("resume them explicitly", second.stderr)


class RoomRecordCompatibilityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cbc-room-record-test-")
        self.addCleanup(self.temp.cleanup)
        self.directory = bootstrap.bootstrap(Path(self.temp.name), "context", "api-feature",
                                             "cbc-feature-lead")
        self.state = self.directory / "state.md"

    def populate_rooms(self):
        content = self.state.read_text().replace(
            "connections:\n", "connections:\n"
            "  parent-report: report-room --as context--api-feature --model test task report-handle\n"
            "  client-peer: peer-room --as context--api-feature --model test task peer-handle\n")
        rows = (
            "report-room | report | parent=context/api-repo child=context/api-feature | issue-1 | "
            "context/api-repo | grant-1 | assigned release accepted\n"
            "peer-room | peer | context/api-feature=context/client-feature | issue-1 contract | "
            "context/api-feature | grant-1 | contract follow-up owned\n")
        self.state.write_text(content.replace("## Room relationships\n", "## Room relationships\n" + rows))

    def test_empty_generated_relationship_section_declares_no_receiving_rooms(self):
        self.assertEqual(handoff.declared_rooms(self.state.read_text()), set())
        directory = handoff.prepare(self.directory, "old-native", "api-feature-lead-test", {})
        self.assertEqual(handoff.verify(directory)["last_handled"], {})

    def test_report_and_peer_rows_survive_prepare_and_final_release(self):
        self.populate_rooms()
        rooms = {"report-room": 7, "peer-room": 9}
        self.assertEqual(handoff.declared_rooms(self.state.read_text()), set(rooms))
        directory = handoff.prepare(self.directory, "old-native", "api-feature-lead-test", rooms)
        self.assertEqual((directory / "state.md").read_bytes(), self.state.read_bytes())
        self.state.write_text(self.state.read_text().replace("phase: setup", "phase: paused"))
        receipt = handoff.release(directory, "old-native", "new-native", "fixture receivers stopped",
                                  {"report-room": 7, "peer-room": 10})
        self.assertEqual((directory / "release-state.md").read_bytes(), self.state.read_bytes())
        self.assertEqual(receipt["last_handled"], {"report-room": 7, "peer-room": 10})
        self.assertEqual(handoff.verify(directory)["transfer_state"], "released")

    def test_metadata_cannot_be_used_as_a_receiving_room_checkpoint(self):
        self.populate_rooms()
        with self.assertRaisesRegex(ValueError, "exactly the declared rooms"):
            handoff.prepare(self.directory, "old-native", "api-feature-lead-test",
                            {"report-room": 7, "peer-room": 9, "grant-1": 0})


if __name__ == "__main__":
    unittest.main()
