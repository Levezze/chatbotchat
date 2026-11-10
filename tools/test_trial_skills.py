"""Project-local WIP trial skill install (decision T-17). No home or daemon writes."""

from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_hierarchy_skills import ROOT, installer, load


trial = load("trial", ROOT / "tools/install_trial_skills.py")
PRESERVED = "cbc-preserved"


class TrialSkillTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cbc-trial-skills-test-")
        self.addCleanup(self.temp.cleanup)
        self.project = Path(self.temp.name) / "worktree"
        self.project.mkdir()

    def targets(self):
        return [self.project / ".claude/skills", self.project / ".agents/skills"]

    def test_installs_prefixed_package_into_both_project_hosts_only(self):
        trial.install(self.project)
        for target in self.targets():
            names = sorted(path.name for path in target.iterdir())
            self.assertEqual(names, sorted(trial.trial_name(name) for name in installer.NAMES))
            for name in installer.NAMES:
                skill = (target / trial.trial_name(name) / "SKILL.md").read_text()
                self.assertRegex(skill, rf"(?m)^name: {re.escape(trial.trial_name(name))}$")
        self.assertEqual(sorted(path.name for path in self.project.iterdir()), [".agents", ".claude"])

    def test_every_name_gets_a_unique_wip_prefix(self):
        self.assertEqual(trial.trial_name("cbc-repo-lead"), "wip-cbc-repo-lead")
        self.assertEqual(trial.trial_name("emperor"), "wip-emperor")
        names = [trial.trial_name(name) for name in installer.NAMES]
        self.assertEqual(len(set(names)), len(names))

    def test_each_entrypoint_tells_the_agent_to_ignore_legacy_skills(self):
        trial.install(self.project)
        for target in self.targets():
            for name in installer.NAMES:
                body = (target / trial.trial_name(name) / "SKILL.md").read_text()
                frontmatter_end = body.index("\n---\n", 4) + 5
                first_line = body[frontmatter_end:].lstrip("\n").splitlines()[0]
                self.assertIn("legacy", first_line)
                self.assertIn("wip-cbc-", first_line)

    def test_no_installed_instruction_routes_to_a_legacy_skill_name(self):
        trial.install(self.project)
        pattern = re.compile(r"(?<![\w-])(" + "|".join(
            re.escape(name) for name in sorted(installer.NAMES, key=len, reverse=True)) + r")(?![\w-])")
        for path in self.project.rglob("*"):
            if path.is_file() and path.suffix in {".md", ".yaml"}:
                with self.subTest(path=path.relative_to(self.project)):
                    self.assertEqual(pattern.findall(path.read_text()), [])

    def test_relative_references_resolve_inside_the_trial_package(self):
        trial.install(self.project)
        checked = 0
        for target in self.targets():
            for document in target.rglob("*.md"):
                for link in re.findall(r"((?:\.\./)+[\w./-]+\.md)", document.read_text()):
                    checked += 1
                    with self.subTest(document=document.relative_to(target), link=link):
                        resolved = (document.parent / link).resolve()
                        self.assertTrue(resolved.is_file())
                        self.assertIn(target.resolve(), resolved.parents)
        self.assertGreater(checked, 20)

    def test_installed_scripts_accept_prefixed_roles_and_record_canonical_ones(self):
        trial.install(self.project)
        scripts = self.targets()[1] / trial.trial_name("cbc-hierarchy") / "scripts"
        workspace = Path(self.temp.name) / "records"
        result = subprocess.run(
            [sys.executable, "-B", "-I", str(scripts / "bootstrap.py"), "--root", str(workspace),
             "--context", "trial-one", "--member", "primarch", "--role", "wip-cbc-repo-lead"],
            capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        record = (workspace / ".cbc/hierarchy/trial-one/primarch/state.md").read_text()
        self.assertRegex(record, r"(?m)^role: cbc-repo-lead$")
        titles = load("trial_titles", scripts / "session_title.py")
        self.assertEqual(titles.resolve_role("wip-cbc-primarch"), ("cbc-repo-lead", "warhammer"))
        self.assertEqual(titles.resolve_role("wip-cbc-feature-lead"), ("cbc-feature-lead", "default"))

    def test_rerun_is_idempotent_and_other_project_skills_survive(self):
        for target in self.targets():
            (target / PRESERVED).mkdir(parents=True)
            (target / PRESERVED / "SKILL.md").write_text("project skill")
        first = trial.install(self.project)
        self.assertTrue(first)
        self.assertEqual(trial.install(self.project), [])
        for target in self.targets():
            self.assertEqual((target / PRESERVED / "SKILL.md").read_text(), "project skill")

    def test_differing_existing_trial_skill_is_refused_without_writes(self):
        claude = self.targets()[0] / trial.trial_name("cbc-hierarchy")
        claude.mkdir(parents=True)
        (claude / "SKILL.md").write_text("hand edited")
        with self.assertRaisesRegex(ValueError, "differs"):
            trial.install(self.project)
        self.assertEqual((claude / "SKILL.md").read_text(), "hand edited")
        self.assertFalse(self.targets()[1].exists())

    def test_symlinked_skills_directory_is_refused(self):
        outside = Path(self.temp.name) / "outside"
        outside.mkdir()
        (self.project / ".claude").mkdir()
        (self.project / ".claude/skills").symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "symlink"):
            trial.install(self.project)
        self.assertEqual(list(outside.iterdir()), [])

    def test_symlinked_host_directory_is_refused(self):
        outside = Path(self.temp.name) / "outside"
        outside.mkdir()
        (self.project / ".agents").symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "symlink"):
            trial.install(self.project)
        self.assertEqual(list(outside.iterdir()), [])
        self.assertFalse((self.project / ".claude").exists())

    def test_cli_check_is_read_only(self):
        result = subprocess.run(
            [sys.executable, "-B", str(ROOT / "tools/install_trial_skills.py"),
             "--project", str(self.project), "--check"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"Would add {2 * len(installer.NAMES)}", result.stdout)
        self.assertEqual(list(self.project.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
